use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use evdev::{Device, EventType, InputEventKind};
use log::{debug, error, info, warn};
use notify_rust::Notification;
use serde_json::json;
use tokio::sync::mpsc;

use crate::ibus_control::{get_current_engine, is_handy_engine, switch_to_handy_engine_verified};
use crate::key_mapping::{
    gdk_keyval_to_evdev, is_modifier_key, modifier_flag_for_key, modifiers_from_held_keys,
    EvdevKeybinding, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER,
};
use crate::settings::Settings;
use crate::utils::launch::open_handy_ui;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

const START_RECORDING_ARM_DELAY_MS: u64 = 120;
const STOP_RECORDING_TIMEOUT_MS: u64 = 20_000;
const ENGINE_SWITCH_VERIFY_TIMEOUT_MS: u64 = 350;
const PENDING_COMMIT_DRAIN_TIMEOUT_MS: u64 = 320;
const PENDING_COMMIT_DRAIN_POLL_MS: u64 = 20;
const SETTINGS_POLL_INTERVAL_MS: u64 = 350;
const FAILURE_NOTIFICATION_COOLDOWN_MS: u64 = 8_000;
const PTT_EVENT_HISTORY_LIMIT: usize = 60;

static PTT_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
static HEALTH_STATE: OnceLock<Mutex<PttRuntimeHealth>> = OnceLock::new();
static PTT_RECENT_EVENTS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static FORCE_REBIND_REQUESTED: AtomicBool = AtomicBool::new(false);

// ── PTT state machine ──────────────────────────────────────────────────

#[derive(Debug)]
enum PttState {
    Idle,
    Pending {
        ptt_session_id: u64,
        released_early: bool,
    },
    Recording {
        ptt_session_id: u64,
        daemon_session_id: u64,
    },
    Stopping {
        ptt_session_id: u64,
        daemon_session_id: u64,
    },
}

enum InternalEvent {
    StartRecording {
        ptt_session_id: u64,
        result: std::result::Result<u64, String>,
    },
    StopRecording {
        ptt_session_id: u64,
        result: std::result::Result<String, String>,
    },
}

// ── Health diagnostics ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PttRuntimeHealth {
    healthy: bool,
    component: String,
    code: String,
    message: String,
    last_success_ms: u64,
    portal_session_ok: bool,
    shortcut_bound: bool,
    portal_bind_fail_count: u64,
    press_while_handy_count: u64,
    release_timeout_fallback_count: u64,
    last_notification_ms: u64,
    current_state: String,
    shortcut_description: String,
    last_start_failure_code: String,
    last_start_failure_message: String,
    last_start_failure_ms: u64,
    last_stop_failure_message: String,
    last_stop_failure_ms: u64,
    pending_commit_session_id: u64,
    pending_commit_mark_ms: u64,
    last_switch_attempt_ms: u64,
    last_switch_confirm_latency_ms: u64,
    last_switch_failure_message: String,
    last_dbus_error: String,
    last_dbus_error_ms: u64,
}

impl Default for PttRuntimeHealth {
    fn default() -> Self {
        Self {
            healthy: false,
            component: "global_shortcuts".to_string(),
            code: "not_initialized".to_string(),
            message: "Global push-to-talk listener not initialized yet".to_string(),
            last_success_ms: 0,
            portal_session_ok: false,
            shortcut_bound: false,
            portal_bind_fail_count: 0,
            press_while_handy_count: 0,
            release_timeout_fallback_count: 0,
            last_notification_ms: 0,
            current_state: "idle".to_string(),
            shortcut_description: String::new(),
            last_start_failure_code: String::new(),
            last_start_failure_message: String::new(),
            last_start_failure_ms: 0,
            last_stop_failure_message: String::new(),
            last_stop_failure_ms: 0,
            pending_commit_session_id: 0,
            pending_commit_mark_ms: 0,
            last_switch_attempt_ms: 0,
            last_switch_confirm_latency_ms: 0,
            last_switch_failure_message: String::new(),
            last_dbus_error: String::new(),
            last_dbus_error_ms: 0,
        }
    }
}

fn health_state() -> &'static Mutex<PttRuntimeHealth> {
    HEALTH_STATE.get_or_init(|| Mutex::new(PttRuntimeHealth::default()))
}

fn ptt_recent_events_state() -> &'static Mutex<VecDeque<String>> {
    PTT_RECENT_EVENTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(PTT_EVENT_HISTORY_LIMIT)))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn push_ptt_event(event: impl Into<String>) {
    let line = format!("{} {}", now_millis(), event.into());
    if let Ok(mut events) = ptt_recent_events_state().lock() {
        events.push_back(line);
        while events.len() > PTT_EVENT_HISTORY_LIMIT {
            let _ = events.pop_front();
        }
    }
}

pub fn ptt_recent_events() -> Vec<String> {
    ptt_recent_events_state()
        .lock()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default()
}

fn mark_health_success(message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.healthy = true;
        health.component = "global_shortcuts".to_string();
        health.code = "ok".to_string();
        health.message = message.to_string();
        health.last_success_ms = now_millis();
        health.portal_session_ok = true;
        health.shortcut_bound = true;
    }
}

fn mark_ptt_state(state: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.current_state = state.to_string();
    }
}

fn mark_health_error(code: &str, message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.healthy = false;
        health.component = "global_shortcuts".to_string();
        health.code = code.to_string();
        health.message = message.to_string();
        if code.starts_with("portal_") || code.starts_with("evdev_") {
            health.portal_session_ok = false;
            if code.contains("bind") || code.contains("permission") {
                health.shortcut_bound = false;
                health.portal_bind_fail_count = health.portal_bind_fail_count.saturating_add(1);
            }
        }
    }
}

fn mark_shortcut_description(description: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.shortcut_description = description.to_string();
    }
}

fn mark_start_failure(code: &str, message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.last_start_failure_code = code.to_string();
        health.last_start_failure_message = message.to_string();
        health.last_start_failure_ms = now_millis();
    }
}

fn clear_start_failure() {
    if let Ok(mut health) = health_state().lock() {
        health.last_start_failure_code.clear();
        health.last_start_failure_message.clear();
        health.last_start_failure_ms = 0;
    }
}

fn mark_stop_failure(message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.last_stop_failure_message = message.to_string();
        health.last_stop_failure_ms = now_millis();
    }
}

fn clear_stop_failure() {
    if let Ok(mut health) = health_state().lock() {
        health.last_stop_failure_message.clear();
        health.last_stop_failure_ms = 0;
    }
}

fn mark_pending_commit(session_id: u64) {
    if let Ok(mut health) = health_state().lock() {
        health.pending_commit_session_id = session_id;
        health.pending_commit_mark_ms = now_millis();
    }
}

fn clear_pending_commit() {
    if let Ok(mut health) = health_state().lock() {
        health.pending_commit_session_id = 0;
        health.pending_commit_mark_ms = 0;
    }
}

fn mark_switch_attempt() {
    if let Ok(mut health) = health_state().lock() {
        health.last_switch_attempt_ms = now_millis();
    }
}

fn mark_switch_confirm(latency_ms: u64) {
    if let Ok(mut health) = health_state().lock() {
        health.last_switch_confirm_latency_ms = latency_ms;
        health.last_switch_failure_message.clear();
    }
}

fn mark_switch_failure(message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.last_switch_failure_message = message.to_string();
    }
}

fn mark_dbus_error(method: &str, message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.last_dbus_error = format!("{}: {}", method, message);
        health.last_dbus_error_ms = now_millis();
    }
}

fn bump_press_while_handy() {
    if let Ok(mut health) = health_state().lock() {
        health.press_while_handy_count = health.press_while_handy_count.saturating_add(1);
    }
}

fn bump_release_timeout_fallback() {
    if let Ok(mut health) = health_state().lock() {
        health.release_timeout_fallback_count =
            health.release_timeout_fallback_count.saturating_add(1);
    }
}

pub fn ptt_diagnostics_tuple() -> (bool, String, String, String, u64, bool, bool, u64, u64, u64) {
    if let Ok(health) = health_state().lock() {
        (
            health.healthy,
            health.component.clone(),
            health.code.clone(),
            health.message.clone(),
            health.last_success_ms,
            health.portal_session_ok,
            health.shortcut_bound,
            health.portal_bind_fail_count,
            health.press_while_handy_count,
            health.release_timeout_fallback_count,
        )
    } else {
        (
            false,
            "global_shortcuts".to_string(),
            "lock_poisoned".to_string(),
            "Failed to read PTT diagnostics".to_string(),
            0,
            false,
            false,
            0,
            0,
            0,
        )
    }
}

pub fn ptt_diagnostics_verbose_json() -> String {
    if let Ok(health) = health_state().lock() {
        let pending_commit_age_ms = if health.pending_commit_session_id == 0 {
            0
        } else {
            now_millis().saturating_sub(health.pending_commit_mark_ms)
        };
        json!({
            "healthy": health.healthy,
            "component": health.component,
            "code": health.code,
            "message": health.message,
            "last_success_ms": health.last_success_ms,
            "listener_session_ok": health.portal_session_ok,
            "shortcut_bound": health.shortcut_bound,
            "bind_fail_count": health.portal_bind_fail_count,
            "press_while_handy_count": health.press_while_handy_count,
            "release_timeout_fallback_count": health.release_timeout_fallback_count,
            "current_state": health.current_state,
            "shortcut_description": health.shortcut_description,
            "last_start_failure_code": health.last_start_failure_code,
            "last_start_failure_message": health.last_start_failure_message,
            "last_start_failure_ms": health.last_start_failure_ms,
            "last_stop_failure_message": health.last_stop_failure_message,
            "last_stop_failure_ms": health.last_stop_failure_ms,
            "pending_commit_session_id": health.pending_commit_session_id,
            "pending_commit_age_ms": pending_commit_age_ms,
            "last_switch_attempt_ms": health.last_switch_attempt_ms,
            "last_switch_confirm_latency_ms": health.last_switch_confirm_latency_ms,
            "last_switch_failure_message": health.last_switch_failure_message,
            "last_dbus_error": health.last_dbus_error,
            "last_dbus_error_ms": health.last_dbus_error_ms,
            "recent_event_count": ptt_recent_events().len(),
        })
        .to_string()
    } else {
        json!({
            "healthy": false,
            "component": "global_shortcuts",
            "code": "lock_poisoned",
            "message": "Failed to read PTT diagnostics",
            "last_success_ms": 0,
            "listener_session_ok": false,
            "shortcut_bound": false,
            "bind_fail_count": 0,
            "press_while_handy_count": 0,
            "release_timeout_fallback_count": 0,
            "current_state": "unknown",
            "shortcut_description": "",
            "last_start_failure_code": "",
            "last_start_failure_message": "Failed to read PTT diagnostics",
            "last_start_failure_ms": 0,
            "last_stop_failure_message": "",
            "last_stop_failure_ms": 0,
            "pending_commit_session_id": 0,
            "pending_commit_age_ms": 0,
            "last_switch_attempt_ms": 0,
            "last_switch_confirm_latency_ms": 0,
            "last_switch_failure_message": "",
            "last_dbus_error": "health_state lock poisoned",
            "last_dbus_error_ms": 0,
            "recent_event_count": 0,
        })
        .to_string()
    }
}

// ── Public entry points ────────────────────────────────────────────────

pub fn start_global_shortcuts_listener() {
    let initial_config = ShortcutConfig::from_settings(&Settings::new());
    mark_health_error("initializing", "Starting global push-to-talk listener");
    mark_ptt_state("initializing");
    push_ptt_event("listener: initializing");

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create runtime for global shortcuts: {}", e);
                mark_health_error(
                    "runtime_init_failed",
                    &format!("Failed to create runtime for global shortcuts: {}", e),
                );
                push_ptt_event(format!("listener: runtime init failed: {}", e));
                return;
            }
        };

        runtime.block_on(async move {
            run_evdev_listener_loop(initial_config).await;
        });
    });
}

pub fn request_shortcut_listener_rebind() {
    FORCE_REBIND_REQUESTED.store(true, Ordering::SeqCst);
}

/// Called from the UI "Authorize Now" button (legacy path).
/// With evdev this is no longer needed — included only for API compatibility.
pub fn authorize_shortcut_interactively_from_ui() -> Result<String> {
    let config = ShortcutConfig::from_settings(&Settings::new());
    let _keybinding = config.resolve().ok_or_else(|| {
        anyhow!(
            "Cannot resolve keybinding for keyval {:#x} + modifiers {:#x}",
            config.keyval,
            config.modifiers
        )
    })?;
    let description = config.human_description();

    // Try opening a keyboard device to validate permissions
    match find_keyboard_devices() {
        Ok(devices) if !devices.is_empty() => {
            request_shortcut_listener_rebind();
            Ok(format!(
                "evdev: {} keyboard(s) accessible, shortcut {} ready",
                devices.len(),
                description
            ))
        }
        Ok(_) => Err(anyhow!(
            "No keyboard devices found in /dev/input/. Is the input group set up?"
        )),
        Err(e) => Err(anyhow!("Cannot access keyboard devices: {}", e)),
    }
}

// ── evdev listener loop ────────────────────────────────────────────────

async fn run_evdev_listener_loop(mut active_config: ShortcutConfig) {
    loop {
        let keybinding = match active_config.resolve() {
            Some(kb) => kb,
            None => {
                let msg = format!(
                    "Unsupported push-to-talk shortcut: keyval {:#x}",
                    active_config.keyval
                );
                mark_health_error("invalid_shortcut", &msg);
                notify_ptt_failure(
                    "Invalid push-to-talk shortcut",
                    "Set a supported shortcut in Handy preferences.",
                );
                // Wait before retrying
                sleep_until_retry_or_rebind(5_000).await;
                active_config = ShortcutConfig::from_settings(&Settings::new());
                continue;
            }
        };

        match run_evdev_session(&active_config, &keybinding).await {
            Ok(()) => {
                // Session ended normally (settings changed, rebind requested)
                info!("evdev session ended normally, restarting");
            }
            Err(e) => {
                warn!("evdev session error: {}", e);
                let code = if e.to_string().contains("Permission denied")
                    || e.to_string().contains("permission")
                {
                    "evdev_permission_denied"
                } else {
                    "evdev_session_error"
                };
                mark_health_error(code, &e.to_string());
                notify_ptt_failure(
                    "Global push-to-talk is unavailable",
                    &format!("Keyboard input error: {}", e),
                );
            }
        }

        active_config = ShortcutConfig::from_settings(&Settings::new());
        sleep_until_retry_or_rebind(2_000).await;
    }
}

async fn run_evdev_session(
    active_config: &ShortcutConfig,
    keybinding: &EvdevKeybinding,
) -> Result<()> {
    let devices = find_keyboard_devices()?;
    if devices.is_empty() {
        return Err(anyhow!(
            "No keyboard devices found. Check /dev/input/ permissions."
        ));
    }

    let description = active_config.human_description();
    let n_devices = devices.len();
    mark_shortcut_description(&description);
    mark_health_success(&format!(
        "Listening on {} keyboard(s) for {}",
        n_devices, description
    ));
    mark_ptt_state("idle");
    info!(
        "evdev: listening on {} keyboard device(s) for PTT shortcut {}",
        n_devices, description
    );

    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();

    // Spawn a reader task for each keyboard device
    let mut reader_handles = Vec::new();
    for device_path in &devices {
        let path = device_path.clone();
        let tx = key_tx.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = read_device_events(path.clone(), tx).await {
                warn!("evdev reader for {:?} ended: {}", path, e);
            }
        });
        reader_handles.push(handle);
    }
    // Drop the original sender so the channel closes when all reader tasks end
    drop(key_tx);

    let mut ptt_state = PttState::Idle;
    let mut config_poll = tokio::time::interval(Duration::from_millis(SETTINGS_POLL_INTERVAL_MS));
    let mut held_modifiers: HashSet<u16> = HashSet::new();

    let loop_result = loop {
        tokio::select! {
            _ = config_poll.tick() => {
                let new_config = ShortcutConfig::from_settings(&Settings::new());
                if new_config != *active_config {
                    info!("Push-to-talk settings changed, restarting evdev session");
                    break Ok(());
                }
                if FORCE_REBIND_REQUESTED.swap(false, Ordering::SeqCst) {
                    info!("Force rebind requested, restarting evdev session");
                    break Ok(());
                }
            }
            maybe_key = key_rx.recv() => {
                let Some(event) = maybe_key else {
                    // All reader tasks exited — keyboard disconnected?
                    break Err(anyhow!(
                        "All keyboard device readers disconnected"
                    ));
                };

                match event {
                    KeyEvent::Press(code) => {
                        if is_modifier_key(code) {
                            held_modifiers.insert(code);
                        } else if code == keybinding.key_code {
                            let current_mods = modifiers_from_held_keys(&held_modifiers);
                            if current_mods == keybinding.modifiers {
                                on_global_pressed(&mut ptt_state, &internal_tx);
                            }
                        }
                    }
                    KeyEvent::Release(code) => {
                        if is_modifier_key(code) {
                            held_modifiers.remove(&code);
                            // If a required modifier was released while PTT is active, treat as release
                            if let Some(flag) = modifier_flag_for_key(code) {
                                if keybinding.modifiers & flag != 0 && !matches!(ptt_state, PttState::Idle) {
                                    on_global_released(&mut ptt_state, &internal_tx);
                                }
                            }
                        } else if code == keybinding.key_code {
                            on_global_released(&mut ptt_state, &internal_tx);
                        }
                    }
                }
            }
            maybe_internal = internal_rx.recv() => {
                let Some(internal) = maybe_internal else {
                    break Err(anyhow!("Internal global shortcut channel closed"));
                };
                handle_internal_event(&mut ptt_state, internal);
            }
        }
    };

    cleanup_state(&mut ptt_state);

    // Cancel all reader tasks
    for handle in reader_handles {
        handle.abort();
    }

    loop_result
}

// ── evdev device management ────────────────────────────────────────────

#[derive(Debug)]
enum KeyEvent {
    Press(u16),
    Release(u16),
}

fn find_keyboard_devices() -> Result<Vec<PathBuf>> {
    let mut keyboards = Vec::new();

    let input_dir = std::fs::read_dir("/dev/input").map_err(|e| {
        anyhow!(
            "Cannot read /dev/input: {}. You may need to add your user to the 'input' group.",
            e
        )
    })?;

    for entry in input_dir.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("event") {
            continue;
        }

        match Device::open(&path) {
            Ok(device) => {
                // Check if this device has keyboard capabilities (EV_KEY with key codes)
                if device.supported_events().contains(EventType::KEY) {
                    let supported_keys = device.supported_keys();
                    let has_keyboard_keys = supported_keys
                        .map(|keys| {
                            // A real keyboard has letter keys
                            keys.contains(evdev::Key::KEY_A)
                                && keys.contains(evdev::Key::KEY_Z)
                                && keys.contains(evdev::Key::KEY_SPACE)
                        })
                        .unwrap_or(false);

                    if has_keyboard_keys {
                        let dev_name = device.name().unwrap_or("unknown");
                        info!("evdev: found keyboard device {:?} ({})", path, dev_name);
                        keyboards.push(path);
                    }
                }
            }
            Err(e) => {
                debug!("evdev: cannot open {:?}: {}", path, e);
            }
        }
    }

    Ok(keyboards)
}

async fn read_device_events(path: PathBuf, tx: mpsc::UnboundedSender<KeyEvent>) -> Result<()> {
    let device = Device::open(&path).map_err(|e| anyhow!("Failed to open {:?}: {}", path, e))?;
    let mut stream = device
        .into_event_stream()
        .map_err(|e| anyhow!("Failed to create event stream for {:?}: {}", path, e))?;

    loop {
        let event = stream
            .next_event()
            .await
            .map_err(|e| anyhow!("Event read error on {:?}: {}", path, e))?;

        if let InputEventKind::Key(key) = event.kind() {
            let code = key.code();
            match event.value() {
                1 => {
                    // Key press
                    if tx.send(KeyEvent::Press(code)).is_err() {
                        break;
                    }
                }
                0 => {
                    // Key release
                    if tx.send(KeyEvent::Release(code)).is_err() {
                        break;
                    }
                }
                2 => {
                    // Key repeat — ignore for PTT
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// ── PTT press/release handlers (preserved from original) ───────────────

fn on_global_pressed(ptt_state: &mut PttState, internal_tx: &mpsc::UnboundedSender<InternalEvent>) {
    if !matches!(ptt_state, PttState::Idle) {
        debug!("Ignoring duplicate global PTT press while not idle");
        return;
    }

    let current_engine = match get_current_engine() {
        Ok(engine) => engine,
        Err(e) => {
            warn!(
                "Global PTT press ignored: failed to read IBus engine: {}",
                e
            );
            mark_health_error("ibus_get_engine_failed", &e.to_string());
            notify_ptt_failure(
                "Cannot start push-to-talk",
                "Failed to read current input source from IBus.",
            );
            return;
        }
    };

    let ptt_session_id = next_ptt_session_id();
    push_ptt_event(format!("ptt:{} pressed", ptt_session_id));
    if let Err(e) = wait_for_pending_commit_drain(
        Duration::from_millis(PENDING_COMMIT_DRAIN_TIMEOUT_MS),
        Duration::from_millis(PENDING_COMMIT_DRAIN_POLL_MS),
    ) {
        warn!(
            "[ptt:{}] Previous pending transcription not yet consumed: {}",
            ptt_session_id, e
        );
        mark_health_error("pending_commit_not_drained", &e);
        notify_ptt_failure(
            "Cannot start push-to-talk",
            "Previous transcription is still being committed. Try again in a moment.",
        );
        push_ptt_event(format!(
            "ptt:{} blocked start because pending commit is not drained: {}",
            ptt_session_id, e
        ));
        return;
    }
    if is_handy_engine(&current_engine) {
        mark_switch_confirm(0);
        push_ptt_event(format!("ptt:{} already on handy source", ptt_session_id));
        bump_press_while_handy();
        push_ptt_event(format!(
            "ptt:{} pressed while handy already active",
            ptt_session_id
        ));
    } else {
        mark_switch_attempt();
        push_ptt_event(format!(
            "ptt:{} switch requested from {}",
            ptt_session_id, current_engine
        ));
        let switch_started = Instant::now();
        let switched_engine = match switch_to_handy_engine_verified(ENGINE_SWITCH_VERIFY_TIMEOUT_MS)
        {
            Ok(engine) => engine,
            Err(e) => {
                warn!(
                    "[ptt:{}] Failed to switch input source to Handy on press: {}",
                    ptt_session_id, e
                );
                mark_switch_failure(&e.to_string());
                mark_health_error("ibus_switch_to_handy_failed", &e.to_string());
                notify_ptt_failure(
                    "Cannot start push-to-talk",
                    "Failed to switch input source to Handy (not confirmed active).",
                );
                push_ptt_event(format!(
                    "ptt:{} failed to switch to handy engine: {}",
                    ptt_session_id, e
                ));
                return;
            }
        };
        mark_switch_confirm(switch_started.elapsed().as_millis() as u64);
        push_ptt_event(format!(
            "ptt:{} switch confirmed to {} ({} ms)",
            ptt_session_id,
            switched_engine,
            switch_started.elapsed().as_millis()
        ));
        info!(
            "[ptt:{}] Pressed; switched to Handy source '{}' from '{}'",
            ptt_session_id, switched_engine, current_engine
        );
    }

    spawn_start_recording(ptt_session_id, internal_tx.clone());
    *ptt_state = PttState::Pending {
        ptt_session_id,
        released_early: false,
    };
    mark_ptt_state("pending");
    clear_pending_commit();
}

fn on_global_released(
    ptt_state: &mut PttState,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) {
    match ptt_state {
        PttState::Idle => {}
        PttState::Pending {
            ptt_session_id,
            released_early,
        } => {
            let current_session = *ptt_session_id;
            info!(
                "[ptt:{}] Released before recording confirmation",
                current_session
            );
            push_ptt_event(format!(
                "ptt:{} released before start confirmed",
                current_session
            ));
            // Mark early release; on_start_recording_result will cancel the stale session.
            *released_early = true;
        }
        PttState::Recording {
            ptt_session_id,
            daemon_session_id,
        } => {
            let current_session = *ptt_session_id;
            let daemon_session = *daemon_session_id;
            info!(
                "[ptt:{}] Released; waiting for StopRecordingSession({})",
                current_session, daemon_session
            );
            push_ptt_event(format!(
                "ptt:{} released; stopping daemon session {}",
                current_session, daemon_session
            ));
            spawn_stop_recording(current_session, daemon_session, internal_tx.clone());
            *ptt_state = PttState::Stopping {
                ptt_session_id: current_session,
                daemon_session_id: daemon_session,
            };
            mark_ptt_state("stopping");
        }
        PttState::Stopping { .. } => {
            debug!("Ignoring duplicate global PTT release while stop is already in progress");
        }
    }
}

fn handle_internal_event(ptt_state: &mut PttState, internal: InternalEvent) {
    match internal {
        InternalEvent::StartRecording {
            ptt_session_id,
            result,
        } => {
            on_start_recording_result(ptt_state, ptt_session_id, result);
        }
        InternalEvent::StopRecording {
            ptt_session_id,
            result,
        } => {
            on_stop_recording_result(ptt_state, ptt_session_id, result);
        }
    }
}

fn on_start_recording_result(
    ptt_state: &mut PttState,
    ptt_session_id: u64,
    result: std::result::Result<u64, String>,
) {
    match ptt_state {
        PttState::Pending {
            ptt_session_id: active_session,
            released_early,
        } if *active_session == ptt_session_id => match result {
            Ok(daemon_session_id) => {
                clear_start_failure();
                clear_stop_failure();
                if *released_early {
                    info!(
                        "[ptt:{}] Start completed after key release; cancelling stale recording",
                        ptt_session_id
                    );
                    // Cancel the recording that started after key release.
                    spawn_cancel_recording(ptt_session_id, "released early");
                    *ptt_state = PttState::Idle;
                    mark_ptt_state("idle");
                    push_ptt_event(format!(
                        "ptt:{} start completed after release; cancelled session {}",
                        ptt_session_id, daemon_session_id
                    ));
                } else {
                    info!(
                        "[ptt:{}] Recording started with daemon session {}",
                        ptt_session_id, daemon_session_id
                    );
                    *ptt_state = PttState::Recording {
                        ptt_session_id,
                        daemon_session_id,
                    };
                    mark_ptt_state("recording");
                    push_ptt_event(format!(
                        "ptt:{} started daemon session {}",
                        ptt_session_id, daemon_session_id
                    ));
                }
            }
            Err(err) => {
                warn!(
                    "[ptt:{}] Failed to start recording: {}",
                    ptt_session_id, err
                );
                let failure_code = extract_start_failure_code(&err);
                mark_start_failure(&failure_code, &err);
                mark_health_error("start_recording_failed", &err);
                notify_ptt_failure(
                    "Cannot start recording",
                    &format!(
                        "Push-to-talk start failed ({})",
                        extract_start_failure_code(&err)
                    ),
                );
                push_ptt_event(format!("ptt:{} start failed: {}", ptt_session_id, err));
                spawn_cancel_recording(ptt_session_id, "start failed");
                *ptt_state = PttState::Idle;
                mark_ptt_state("idle");
                clear_pending_commit();
            }
        },
        _ => {
            if result.is_ok() {
                warn!(
                    "[ptt:{}] Received stale start success, cancelling recording to avoid orphan state",
                    ptt_session_id
                );
                spawn_cancel_recording(ptt_session_id, "stale start success");
                push_ptt_event(format!(
                    "ptt:{} stale start success cancelled",
                    ptt_session_id
                ));
            } else {
                debug!(
                    "[ptt:{}] Ignoring stale start failure for inactive session",
                    ptt_session_id
                );
                push_ptt_event(format!(
                    "ptt:{} stale start failure ignored",
                    ptt_session_id
                ));
            }
        }
    }
}

fn on_stop_recording_result(
    ptt_state: &mut PttState,
    ptt_session_id: u64,
    result: std::result::Result<String, String>,
) {
    match ptt_state {
        PttState::Stopping {
            ptt_session_id: active_session,
            daemon_session_id,
        } if *active_session == ptt_session_id => {
            match result {
                Ok(text) => {
                    info!(
                        "[ptt:{}] StopRecordingSession({}) completed",
                        ptt_session_id, daemon_session_id
                    );
                    clear_stop_failure();
                    mark_pending_commit(*daemon_session_id);
                    push_ptt_event(format!(
                        "ptt:{} stopped daemon session {} (text_len={})",
                        ptt_session_id,
                        daemon_session_id,
                        text.chars().count()
                    ));
                    push_ptt_event(format!(
                        "ptt:{} stop-complete for session {}; commit is delivered by engine-side pending commit listener",
                        ptt_session_id, daemon_session_id
                    ));
                    *ptt_state = PttState::Idle;
                    mark_ptt_state("idle");
                    return;
                }
                Err(err) => {
                    warn!(
                        "[ptt:{}] StopRecordingSession({}) failed: {}",
                        ptt_session_id, daemon_session_id, err
                    );
                    if err.contains("timed out") {
                        bump_release_timeout_fallback();
                    }
                    mark_health_error("stop_recording_failed", &err);
                    mark_stop_failure(&err);
                    push_ptt_event(format!(
                        "ptt:{} stop failed for daemon session {}: {}",
                        ptt_session_id, daemon_session_id, err
                    ));
                }
            }

            clear_pending_commit();
            *ptt_state = PttState::Idle;
            mark_ptt_state("idle");
            push_ptt_event(format!(
                "ptt:{} stop result handled (failure path)",
                ptt_session_id
            ));
        }
        _ => {
            debug!(
                "[ptt:{}] Ignoring stale stop result for inactive session",
                ptt_session_id
            );
            push_ptt_event(format!("ptt:{} stale stop result ignored", ptt_session_id));
        }
    }
}

fn cleanup_state(ptt_state: &mut PttState) {
    match ptt_state {
        PttState::Idle => {}
        PttState::Pending { ptt_session_id, .. } => {
            let sid = *ptt_session_id;
            spawn_cancel_recording(sid, "cleanup");
        }
        PttState::Recording {
            ptt_session_id,
            daemon_session_id: _,
        } => {
            let sid = *ptt_session_id;
            spawn_cancel_recording(sid, "cleanup");
        }
        PttState::Stopping {
            ptt_session_id,
            daemon_session_id: _,
        } => {
            let sid = *ptt_session_id;
            spawn_cancel_recording(sid, "cleanup after stop pending");
        }
    }

    *ptt_state = PttState::Idle;
    mark_ptt_state("idle");
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn spawn_start_recording(ptt_session_id: u64, tx: mpsc::UnboundedSender<InternalEvent>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(START_RECORDING_ARM_DELAY_MS));
        let result = call_handy_start_recording_session();
        let _ = tx.send(InternalEvent::StartRecording {
            ptt_session_id,
            result,
        });
    });
}

fn spawn_stop_recording(
    ptt_session_id: u64,
    daemon_session_id: u64,
    tx: mpsc::UnboundedSender<InternalEvent>,
) {
    std::thread::spawn(move || {
        let result = match call_handy_stop_recording_session_with_timeout(
            daemon_session_id,
            Duration::from_millis(STOP_RECORDING_TIMEOUT_MS),
        ) {
            Ok(text) => Ok(text),
            Err(stop_err) => {
                let cancel_result = call_handy_method_no_args("CancelRecording");
                Err(match cancel_result {
                    Ok(()) => format!("{}; fallback CancelRecording succeeded", stop_err),
                    Err(cancel_err) => {
                        format!(
                            "{}; fallback CancelRecording failed: {}",
                            stop_err, cancel_err
                        )
                    }
                })
            }
        };
        let _ = tx.send(InternalEvent::StopRecording {
            ptt_session_id,
            result,
        });
    });
}

fn spawn_cancel_recording(session_id: u64, reason: &'static str) {
    std::thread::spawn(move || match call_handy_method_no_args("CancelRecording") {
        Ok(()) => {
            info!("[ptt:{}] Cancelled recording ({})", session_id, reason);
        }
        Err(e) => {
            warn!(
                "[ptt:{}] Failed to cancel recording ({}): {}",
                session_id, reason, e
            );
        }
    });
}

fn call_handy_method_no_args(method: &str) -> std::result::Result<(), String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| {
        let msg = format!("Failed to open session bus: {}", e);
        mark_dbus_error(method, &msg);
        msg
    })?;
    conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        method,
        &(),
    )
    .map_err(|e| {
        let msg = format!("{} call failed: {}", method, e);
        mark_dbus_error(method, &msg);
        msg
    })?;
    Ok(())
}

fn call_handy_start_recording_session() -> std::result::Result<u64, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| {
        let msg = format!("Failed to open session bus: {}", e);
        mark_dbus_error("StartRecordingSession", &msg);
        msg
    })?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StartRecordingSession",
            &(),
        )
        .map_err(|e| {
            let msg = format!("StartRecordingSession call failed: {}", e);
            mark_dbus_error("StartRecordingSession", &msg);
            msg
        })?;
    reply.body().deserialize::<u64>().map_err(|e| {
        let msg = format!("StartRecordingSession decode failed: {}", e);
        mark_dbus_error("StartRecordingSession", &msg);
        msg
    })
}

fn call_handy_peek_pending_commit_session() -> std::result::Result<u64, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| {
        let msg = format!("Failed to open session bus: {}", e);
        mark_dbus_error("PeekPendingCommitSession", &msg);
        msg
    })?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "PeekPendingCommitSession",
            &(),
        )
        .map_err(|e| {
            let msg = format!("PeekPendingCommitSession call failed: {}", e);
            mark_dbus_error("PeekPendingCommitSession", &msg);
            msg
        })?;
    reply.body().deserialize::<u64>().map_err(|e| {
        let msg = format!("PeekPendingCommitSession decode failed: {}", e);
        mark_dbus_error("PeekPendingCommitSession", &msg);
        msg
    })
}

fn wait_for_pending_commit_drain(
    timeout: Duration,
    poll_interval: Duration,
) -> std::result::Result<(), String> {
    let start = Instant::now();
    let mut last_pending_session = 0_u64;
    let mut last_error = String::new();

    loop {
        match call_handy_peek_pending_commit_session() {
            Ok(pending_session) => {
                last_pending_session = pending_session;
                if pending_session == 0 {
                    return Ok(());
                }
            }
            Err(e) => {
                last_error = e;
            }
        }

        if start.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(poll_interval);
    }

    Err(format!(
        "Pending commit did not drain within {} ms (last_pending_session={} last_error='{}')",
        timeout.as_millis(),
        last_pending_session,
        last_error
    ))
}

fn call_handy_stop_recording_session(session_id: u64) -> std::result::Result<String, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| {
        let msg = format!("Failed to open session bus: {}", e);
        mark_dbus_error("StopRecordingSession", &msg);
        msg
    })?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StopRecordingSession",
            &(session_id,),
        )
        .map_err(|e| {
            let msg = format!("StopRecordingSession call failed: {}", e);
            mark_dbus_error("StopRecordingSession", &msg);
            msg
        })?;
    reply.body().deserialize::<String>().map_err(|e| {
        let msg = format!("StopRecordingSession decode failed: {}", e);
        mark_dbus_error("StopRecordingSession", &msg);
        msg
    })
}

fn call_handy_stop_recording_session_with_timeout(
    session_id: u64,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(call_handy_stop_recording_session(session_id));
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            let msg = format!(
                "StopRecordingSession call timed out after {} ms",
                timeout.as_millis()
            );
            mark_dbus_error("StopRecordingSession", &msg);
            Err(msg)
        }
        Err(RecvTimeoutError::Disconnected) => {
            let msg = "StopRecordingSession call worker disconnected before returning".to_string();
            mark_dbus_error("StopRecordingSession", &msg);
            Err(msg)
        }
    }
}

fn extract_start_failure_code(err: &str) -> String {
    let needle = "Failed to start recording (";
    if let Some(start) = err.find(needle) {
        let code_start = start + needle.len();
        if let Some(end_rel) = err[code_start..].find("):") {
            return err[code_start..code_start + end_rel].trim().to_string();
        }
    }
    "start_recording_failed".to_string()
}

fn notify_ptt_failure(summary: &str, body: &str) {
    let now = now_millis();
    if let Ok(mut health) = health_state().lock() {
        if now.saturating_sub(health.last_notification_ms) < FAILURE_NOTIFICATION_COOLDOWN_MS {
            return;
        }
        health.last_notification_ms = now;
    }

    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let notification = Notification::new()
            .summary(&summary)
            .body(&body)
            .action("default", "Show Diagnostics")
            .show();
        if let Ok(handle) = notification {
            handle.wait_for_action(|action| {
                if action == "default" || action == "clicked" {
                    if let Err(e) = open_handy_ui(None) {
                        error!("Failed to open Handy diagnostics UI: {}", e);
                    }
                }
            });
        }
    });
}

fn next_ptt_session_id() -> u64 {
    PTT_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ── Shortcut config ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShortcutConfig {
    keyval: u32,
    modifiers: u32,
}

impl ShortcutConfig {
    fn from_settings(settings: &Settings) -> Self {
        Self {
            keyval: normalize_keyval(settings.push_to_talk_keyval()),
            modifiers: settings.push_to_talk_modifiers(),
        }
    }

    /// Resolve to an evdev keybinding.
    fn resolve(&self) -> Option<EvdevKeybinding> {
        crate::key_mapping::resolve_keybinding(self.keyval, self.modifiers)
    }

    /// Human-readable description of the shortcut.
    fn human_description(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.modifiers & MOD_CTRL != 0 {
            parts.push("Ctrl");
        }
        if self.modifiers & MOD_ALT != 0 {
            parts.push("Alt");
        }
        if self.modifiers & MOD_SHIFT != 0 {
            parts.push("Shift");
        }
        if self.modifiers & MOD_SUPER != 0 {
            parts.push("Super");
        }

        let key_name = gdk_keyval_to_evdev(self.keyval)
            .map(|code| format!("{:?}", evdev::Key(code)))
            .unwrap_or_else(|| format!("keyval_{:#x}", self.keyval));

        parts.push(&key_name);
        // Need to collect since key_name is a local
        let parts_owned: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
        parts_owned.join("+")
    }
}

fn normalize_keyval(keyval: u32) -> u32 {
    if (b'A' as u32..=b'Z' as u32).contains(&keyval) {
        keyval + (b'a' - b'A') as u32
    } else {
        keyval
    }
}

async fn sleep_until_retry_or_rebind(total_delay_ms: u64) {
    let mut remaining = total_delay_ms;
    while remaining > 0 {
        if FORCE_REBIND_REQUESTED.swap(false, Ordering::SeqCst) {
            return;
        }
        let step = remaining.min(200);
        tokio::time::sleep(Duration::from_millis(step)).await;
        remaining -= step;
    }
}

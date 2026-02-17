use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use glib::translate::from_glib;
use gtk4::gdk;
use log::{debug, error, info, warn};
use notify_rust::Notification;
use zbus::proxy::SignalStream;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use crate::ibus_control::{
    get_current_engine, is_handy_engine, set_global_engine, HANDY_ENGINE_NAME,
};
use crate::settings::Settings;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

const SHORTCUT_ID: &str = "push_to_talk";

const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 4;
const MOD_ALT: u32 = 8;
const MOD_SUPER: u32 = 64;

const START_RECORDING_ARM_DELAY_MS: u64 = 120;
const SETTINGS_POLL_INTERVAL_MS: u64 = 350;
const RELEASE_WATCHDOG_DELAY_MS: u64 = 700;
const FAILURE_NOTIFICATION_COOLDOWN_MS: u64 = 8_000;

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);
static PTT_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
static HEALTH_STATE: OnceLock<Mutex<PttRuntimeHealth>> = OnceLock::new();
static LAST_NON_HANDY_ENGINE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

type ShortcutOptions = HashMap<String, OwnedValue>;
type PortalShortcutList = Vec<(String, ShortcutOptions)>;

#[derive(Debug)]
enum PttState {
    Idle,
    Pending {
        session_id: u64,
        restore_engine: Option<String>,
        released_early: bool,
    },
    Recording {
        session_id: u64,
        restore_engine: Option<String>,
    },
}

enum InternalEvent {
    StartRecordingResult {
        session_id: u64,
        result: std::result::Result<(), String>,
    },
}

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
        }
    }
}

fn health_state() -> &'static Mutex<PttRuntimeHealth> {
    HEALTH_STATE.get_or_init(|| Mutex::new(PttRuntimeHealth::default()))
}

fn last_non_handy_engine_state() -> &'static Mutex<Option<String>> {
    LAST_NON_HANDY_ENGINE.get_or_init(|| Mutex::new(None))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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

fn mark_health_error(code: &str, message: &str) {
    if let Ok(mut health) = health_state().lock() {
        health.healthy = false;
        health.component = "global_shortcuts".to_string();
        health.code = code.to_string();
        health.message = message.to_string();
        if code.starts_with("portal_") {
            health.portal_session_ok = false;
            if code.contains("bind") {
                health.shortcut_bound = false;
                health.portal_bind_fail_count = health.portal_bind_fail_count.saturating_add(1);
            }
        }
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

pub fn start_global_shortcuts_listener() {
    let initial_config = ShortcutConfig::from_settings(&Settings::new());
    mark_health_error("initializing", "Starting global push-to-talk listener");

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
                return;
            }
        };

        runtime.block_on(async move {
            run_listener_loop(initial_config).await;
        });
    });
}

async fn run_listener_loop(mut active_config: ShortcutConfig) {
    loop {
        match run_shortcut_session(active_config).await {
            Ok(()) => {}
            Err(e) => {
                warn!("Global shortcut session ended: {}", e);
                mark_health_error("portal_session_ended", &e.to_string());
                notify_ptt_failure(
                    "Global push-to-talk is unavailable",
                    &format!("Portal session failed: {}", e),
                );
            }
        }

        active_config = ShortcutConfig::from_settings(&Settings::new());
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn run_shortcut_session(active_config: ShortcutConfig) -> Result<()> {
    let trigger = match active_config.trigger() {
        Ok(trigger) => trigger,
        Err(reason) => {
            let message = format!("Unsupported push-to-talk shortcut: {}", reason);
            mark_health_error("invalid_shortcut", &message);
            notify_ptt_failure(
                "Invalid push-to-talk shortcut",
                "Set a shortcut like Ctrl/Alt/Super + key in Handy preferences.",
            );
            return Err(anyhow!(message));
        }
    };

    let connection = Connection::session().await?;
    let portal_proxy = Proxy::new(&connection, PORTAL_BUS, PORTAL_PATH, SHORTCUTS_IFACE).await?;

    let session_handle = create_session(&portal_proxy, &connection)
        .await
        .inspect_err(|e| {
            mark_health_error("portal_create_session_failed", &e.to_string());
        })?;
    if let Ok(mut health) = health_state().lock() {
        health.portal_session_ok = true;
    }

    if let Err(e) = bind_shortcut(&portal_proxy, &connection, &session_handle, &trigger).await {
        mark_health_error("portal_bind_failed", &e.to_string());
        notify_ptt_failure(
            "Global push-to-talk registration failed",
            "Failed to bind configured shortcut. Open Handy diagnostics.",
        );
        close_session(&connection, &session_handle).await;
        return Err(e);
    }
    if let Ok(mut health) = health_state().lock() {
        health.shortcut_bound = true;
    }
    let effective_trigger = match list_shortcuts(&portal_proxy, &connection, &session_handle).await
    {
        Ok(v) => v,
        Err(e) => {
            mark_health_error("portal_list_shortcuts_failed", &e.to_string());
            close_session(&connection, &session_handle).await;
            return Err(e);
        }
    };
    if let Some(actual_trigger) = effective_trigger {
        if !triggers_match(&actual_trigger, &trigger) {
            mark_health_error(
                "portal_trigger_mismatch",
                &format!(
                    "Requested '{}', portal active '{}'",
                    trigger, actual_trigger
                ),
            );
            notify_ptt_failure(
                "Global shortcut mismatch",
                &format!(
                    "Configured '{}', but portal bound '{}'",
                    trigger, actual_trigger
                ),
            );
        } else {
            mark_health_success(&format!("Global shortcut active: {}", trigger));
        }
    } else {
        mark_health_error(
            "portal_missing_shortcut",
            "Portal did not return an active push-to-talk shortcut",
        );
        notify_ptt_failure(
            "Global shortcut not active",
            "Portal did not activate the configured push-to-talk shortcut.",
        );
        close_session(&connection, &session_handle).await;
        return Err(anyhow!("Portal did not return active shortcut"));
    }
    if let Err(e) = list_shortcuts(&portal_proxy, &connection, &session_handle).await {
        warn!("Failed to query registered shortcuts: {}", e);
    }
    info!("Global push-to-talk registered with trigger {}", trigger);

    let (internal_tx, mut internal_rx) = tokio::sync::mpsc::unbounded_channel::<InternalEvent>();
    let mut signal_stream = portal_proxy.receive_all_signals().await?;
    let mut config_poll = tokio::time::interval(Duration::from_millis(SETTINGS_POLL_INTERVAL_MS));
    let mut ptt_state = PttState::Idle;

    let loop_result = loop {
        tokio::select! {
            _ = config_poll.tick() => {
                if ShortcutConfig::from_settings(&Settings::new()) != active_config {
                    info!("Push-to-talk settings changed, rebinding global shortcut");
                    break Ok(());
                }
            }
            maybe_signal = signal_stream.next() => {
                let Some(signal_msg) = maybe_signal else {
                    break Err(anyhow!("Global shortcut signal stream closed"));
                };

                let header = signal_msg.header();
                let Some(member) = header.member() else {
                    continue;
                };

                match member.as_str() {
                    "Activated" => {
                        let (handle, shortcut_id, _timestamp, _options): (
                            OwnedObjectPath,
                            String,
                            u32,
                            ShortcutOptions,
                        ) = signal_msg.body().deserialize()?;
                        if handle == session_handle && shortcut_id == SHORTCUT_ID {
                            on_global_pressed(&mut ptt_state, &internal_tx);
                        }
                    }
                    "Deactivated" => {
                        let (handle, shortcut_id, _timestamp, _options): (
                            OwnedObjectPath,
                            String,
                            u32,
                            ShortcutOptions,
                        ) = signal_msg.body().deserialize()?;
                        if handle == session_handle && shortcut_id == SHORTCUT_ID {
                            on_global_released(&mut ptt_state);
                        }
                    }
                    "ShortcutsChanged" => {
                        let (handle, shortcuts): (OwnedObjectPath, PortalShortcutList) =
                            signal_msg.body().deserialize()?;
                        if handle == session_handle {
                            log_shortcuts_changed(shortcuts);
                        }
                    }
                    _ => {}
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
    close_session(&connection, &session_handle).await;

    loop_result
}

fn on_global_pressed(
    ptt_state: &mut PttState,
    internal_tx: &tokio::sync::mpsc::UnboundedSender<InternalEvent>,
) {
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

    let session_id = next_ptt_session_id();
    let restore_engine = if is_handy_engine(&current_engine) {
        bump_press_while_handy();
        let restore_engine = last_non_handy_engine_state()
            .lock()
            .ok()
            .and_then(|engine| engine.clone());

        let Some(restore_engine) = restore_engine else {
            warn!(
                "[ptt:{}] Pressed while Handy source active, but no previous source to restore",
                session_id
            );
            mark_health_error(
                "missing_restore_engine",
                "Pressed while Handy active but no non-Handy source is known",
            );
            notify_ptt_failure(
                "Cannot start push-to-talk",
                "No previous input source to restore. Switch to a non-Handy source once.",
            );
            return;
        };
        info!(
            "[ptt:{}] Pressed while Handy source already active; will restore source '{}'",
            session_id, restore_engine
        );
        Some(restore_engine)
    } else {
        if let Ok(mut state) = last_non_handy_engine_state().lock() {
            *state = Some(current_engine.clone());
        }
        if let Err(e) = set_global_engine(HANDY_ENGINE_NAME) {
            warn!(
                "[ptt:{}] Failed to switch input source to Handy on press: {}",
                session_id, e
            );
            mark_health_error("ibus_switch_to_handy_failed", &e.to_string());
            notify_ptt_failure(
                "Cannot start push-to-talk",
                "Failed to switch input source to Handy.",
            );
            return;
        }
        info!(
            "[ptt:{}] Pressed; switched to Handy source from '{}'",
            session_id, current_engine
        );
        Some(current_engine)
    };

    spawn_start_recording(session_id, internal_tx.clone());
    *ptt_state = PttState::Pending {
        session_id,
        restore_engine,
        released_early: false,
    };
}

fn on_global_released(ptt_state: &mut PttState) {
    match ptt_state {
        PttState::Idle => {}
        PttState::Pending {
            session_id,
            restore_engine,
            released_early,
        } => {
            let current_session = *session_id;
            info!(
                "[ptt:{}] Released before recording confirmation",
                current_session
            );
            if let Some(engine_name) = restore_engine.as_deref() {
                if restore_engine_for_session(current_session, engine_name, "pending release")
                    .is_err()
                {
                    spawn_cancel_recording(current_session, "failed restore after early release");
                }
            }
            *released_early = true;
        }
        PttState::Recording {
            session_id,
            restore_engine,
        } => {
            let current_session = *session_id;
            info!("[ptt:{}] Released", current_session);
            if let Some(engine_name) = restore_engine.as_deref() {
                if restore_engine_for_session(current_session, engine_name, "release").is_err() {
                    spawn_cancel_recording(current_session, "failed restore on release");
                } else {
                    spawn_release_watchdog(current_session);
                }
            } else {
                spawn_release_watchdog(current_session);
            }
            *ptt_state = PttState::Idle;
        }
    }
}

fn handle_internal_event(ptt_state: &mut PttState, internal: InternalEvent) {
    match internal {
        InternalEvent::StartRecordingResult { session_id, result } => {
            on_start_recording_result(ptt_state, session_id, result);
        }
    }
}

fn on_start_recording_result(
    ptt_state: &mut PttState,
    session_id: u64,
    result: std::result::Result<(), String>,
) {
    match ptt_state {
        PttState::Pending {
            session_id: active_session,
            restore_engine,
            released_early,
        } if *active_session == session_id => match result {
            Ok(()) => {
                if *released_early {
                    info!(
                        "[ptt:{}] Start completed after key release; cancelling stale recording",
                        session_id
                    );
                    if let Some(engine_name) = restore_engine.as_deref() {
                        if restore_engine_for_session(
                            session_id,
                            engine_name,
                            "late start after release",
                        )
                        .is_err()
                        {
                            warn!(
                                "[ptt:{}] Source restore still failing after late start",
                                session_id
                            );
                        }
                    }
                    spawn_cancel_recording(session_id, "late start after release");
                    *ptt_state = PttState::Idle;
                } else {
                    info!("[ptt:{}] Recording started", session_id);
                    let restore_engine = restore_engine.clone();
                    *ptt_state = PttState::Recording {
                        session_id,
                        restore_engine,
                    };
                }
            }
            Err(err) => {
                warn!("[ptt:{}] Failed to start recording: {}", session_id, err);
                mark_health_error("start_recording_failed", &err);
                notify_ptt_failure(
                    "Cannot start recording",
                    "Handy failed to start recording for push-to-talk.",
                );
                if let Some(engine_name) = restore_engine.as_deref() {
                    if restore_engine_for_session(session_id, engine_name, "start failure").is_err()
                    {
                        spawn_cancel_recording(session_id, "start failure + restore failure");
                    }
                }
                *ptt_state = PttState::Idle;
            }
        },
        _ => {
            if result.is_ok() {
                warn!(
                    "[ptt:{}] Received stale start success, cancelling recording to avoid orphan state",
                    session_id
                );
                spawn_cancel_recording(session_id, "stale start success");
            } else {
                debug!(
                    "[ptt:{}] Ignoring stale start failure for inactive session",
                    session_id
                );
            }
        }
    }
}

fn cleanup_state(ptt_state: &mut PttState) {
    match ptt_state {
        PttState::Idle => {}
        PttState::Pending {
            session_id,
            restore_engine,
            ..
        } => {
            if let Some(engine_name) = restore_engine.as_deref() {
                if restore_engine_for_session(*session_id, engine_name, "session cleanup").is_err()
                {
                    spawn_cancel_recording(*session_id, "cleanup restore failure");
                }
            }
        }
        PttState::Recording {
            session_id,
            restore_engine,
        } => {
            if let Some(engine_name) = restore_engine.as_deref() {
                if restore_engine_for_session(*session_id, engine_name, "session cleanup").is_err()
                {
                    spawn_cancel_recording(*session_id, "cleanup restore failure");
                }
            }
        }
    }

    *ptt_state = PttState::Idle;
}

fn restore_engine_for_session(session_id: u64, restore_engine: &str, reason: &str) -> Result<()> {
    match set_global_engine(restore_engine) {
        Ok(()) => {
            info!(
                "[ptt:{}] Restored source '{}' ({})",
                session_id, restore_engine, reason
            );
            Ok(())
        }
        Err(e) => {
            warn!(
                "[ptt:{}] Failed to restore source '{}' ({}): {}",
                session_id, restore_engine, reason, e
            );
            mark_health_error("ibus_restore_engine_failed", &e.to_string());
            notify_ptt_failure(
                "Input source restore failed",
                "Handy could not restore your previous input source after push-to-talk.",
            );
            Err(e)
        }
    }
}

fn spawn_start_recording(session_id: u64, tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(START_RECORDING_ARM_DELAY_MS));
        let result = call_handy_method_no_args("StartRecording").map(|_| ());
        let _ = tx.send(InternalEvent::StartRecordingResult { session_id, result });
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

fn spawn_release_watchdog(session_id: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(RELEASE_WATCHDOG_DELAY_MS));
        match call_handy_get_state() {
            Ok((is_recording, _has_model)) if is_recording => {
                bump_release_timeout_fallback();
                warn!(
                    "[ptt:{}] Recording still active after release; forcing cancel",
                    session_id
                );
                if let Err(e) = call_handy_method_no_args("CancelRecording") {
                    warn!(
                        "[ptt:{}] Failed to cancel recording from release watchdog: {}",
                        session_id, e
                    );
                }
            }
            Ok(_) => {}
            Err(e) => {
                debug!(
                    "[ptt:{}] Failed to read recording state during release watchdog: {}",
                    session_id, e
                );
            }
        }
    });
}

fn call_handy_method_no_args(method: &str) -> std::result::Result<(), String> {
    let conn = zbus::blocking::Connection::session()
        .map_err(|e| format!("Failed to open session bus: {}", e))?;
    conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        method,
        &(),
    )
    .map_err(|e| format!("{} call failed: {}", method, e))?;
    Ok(())
}

fn call_handy_get_state() -> std::result::Result<(bool, bool), String> {
    let conn = zbus::blocking::Connection::session()
        .map_err(|e| format!("Failed to open session bus: {}", e))?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "GetState",
            &(),
        )
        .map_err(|e| format!("GetState call failed: {}", e))?;
    reply
        .body()
        .deserialize::<(bool, bool)>()
        .map_err(|e| format!("GetState decode failed: {}", e))
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
                    if let Err(e) = std::process::Command::new("/usr/bin/handy").spawn() {
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

fn log_shortcuts_changed(shortcuts: PortalShortcutList) {
    for (shortcut_id, options) in shortcuts {
        if shortcut_id != SHORTCUT_ID {
            continue;
        }

        if let Some(value) = options.get("trigger_description") {
            if let Ok(cloned) = value.try_clone() {
                if let Ok(description) = String::try_from(cloned) {
                    info!("Global shortcut updated by portal: {}", description);
                    mark_health_success(&format!("Global shortcut updated: {}", description));
                    return;
                }
            }
        }

        info!("Global shortcut updated by portal");
        mark_health_success("Global shortcut updated by portal");
        return;
    }
}

async fn create_session(
    portal_proxy: &Proxy<'_>,
    connection: &Connection,
) -> Result<OwnedObjectPath> {
    let handle_token = new_token("handy_gs_create");
    let request_handle = request_handle_for_token(connection, &handle_token)?;
    let request_proxy = Proxy::new(
        connection,
        PORTAL_BUS,
        request_handle.as_str(),
        REQUEST_IFACE,
    )
    .await?;
    let mut response_stream = request_proxy.receive_signal("Response").await?;

    let mut options: ShortcutOptions = HashMap::new();
    options.insert(
        "handle_token".to_string(),
        Value::from(handle_token).try_into()?,
    );
    options.insert(
        "session_handle_token".to_string(),
        Value::from(new_token("handy_gs_session")).try_into()?,
    );

    let returned_request_handle: OwnedObjectPath =
        portal_proxy.call("CreateSession", &(options)).await?;
    if returned_request_handle != request_handle {
        warn!(
            "Portal returned unexpected request handle {}; expected {}",
            returned_request_handle, request_handle
        );
    }

    let (response_code, mut response_data) = await_request_response(&mut response_stream).await?;
    ensure_success_response("GlobalShortcuts CreateSession", response_code)?;

    let session_handle_value = response_data
        .remove("session_handle")
        .ok_or_else(|| anyhow!("CreateSession response missing session_handle"))?;

    Ok(session_handle_value.try_into()?)
}

async fn bind_shortcut(
    portal_proxy: &Proxy<'_>,
    connection: &Connection,
    session_handle: &OwnedObjectPath,
    trigger: &str,
) -> Result<()> {
    let handle_token = new_token("handy_gs_bind");
    let request_handle = request_handle_for_token(connection, &handle_token)?;
    let request_proxy = Proxy::new(
        connection,
        PORTAL_BUS,
        request_handle.as_str(),
        REQUEST_IFACE,
    )
    .await?;
    let mut response_stream = request_proxy.receive_signal("Response").await?;

    let mut shortcut_options: ShortcutOptions = HashMap::new();
    shortcut_options.insert(
        "description".to_string(),
        Value::from("Handy push-to-talk").try_into()?,
    );
    shortcut_options.insert(
        "preferred_trigger".to_string(),
        Value::from(trigger).try_into()?,
    );
    let shortcuts = vec![(SHORTCUT_ID.to_string(), shortcut_options)];

    let mut bind_options: ShortcutOptions = HashMap::new();
    bind_options.insert(
        "handle_token".to_string(),
        Value::from(handle_token).try_into()?,
    );

    let returned_request_handle: OwnedObjectPath = portal_proxy
        .call(
            "BindShortcuts",
            &(session_handle, shortcuts, String::new(), bind_options),
        )
        .await?;
    if returned_request_handle != request_handle {
        warn!(
            "Portal returned unexpected bind request handle {}; expected {}",
            returned_request_handle, request_handle
        );
    }

    let (response_code, _) = await_request_response(&mut response_stream).await?;
    ensure_success_response("GlobalShortcuts BindShortcuts", response_code)
}

async fn list_shortcuts(
    portal_proxy: &Proxy<'_>,
    connection: &Connection,
    session_handle: &OwnedObjectPath,
) -> Result<Option<String>> {
    let handle_token = new_token("handy_gs_list");
    let request_handle = request_handle_for_token(connection, &handle_token)?;
    let request_proxy = Proxy::new(
        connection,
        PORTAL_BUS,
        request_handle.as_str(),
        REQUEST_IFACE,
    )
    .await?;
    let mut response_stream = request_proxy.receive_signal("Response").await?;

    let mut list_options: ShortcutOptions = HashMap::new();
    list_options.insert(
        "handle_token".to_string(),
        Value::from(handle_token).try_into()?,
    );

    let returned_request_handle: OwnedObjectPath = portal_proxy
        .call("ListShortcuts", &(session_handle, list_options))
        .await?;
    if returned_request_handle != request_handle {
        warn!(
            "Portal returned unexpected list request handle {}; expected {}",
            returned_request_handle, request_handle
        );
    }

    let (response_code, mut response_data) = await_request_response(&mut response_stream).await?;
    ensure_success_response("GlobalShortcuts ListShortcuts", response_code)?;

    if let Some(shortcuts_value) = response_data.remove("shortcuts") {
        if let Ok(shortcuts) = PortalShortcutList::try_from(shortcuts_value) {
            let active = extract_trigger_description(&shortcuts);
            log_shortcuts_changed(shortcuts);
            return Ok(active);
        }
    }

    Ok(None)
}

async fn close_session(connection: &Connection, session_handle: &OwnedObjectPath) {
    let session_proxy = match Proxy::new(
        connection,
        PORTAL_BUS,
        session_handle.as_str(),
        SESSION_IFACE,
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!("Failed to create portal session proxy for close: {}", e);
            return;
        }
    };

    let close_result: zbus::Result<()> = session_proxy.call("Close", &()).await;
    if let Err(e) = close_result {
        warn!("Failed to close portal shortcut session: {}", e);
    }
}

async fn await_request_response(
    response_stream: &mut SignalStream<'_>,
) -> Result<(u32, ShortcutOptions)> {
    let response_msg = response_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("Portal request response stream ended"))?;
    Ok(response_msg
        .body()
        .deserialize::<(u32, ShortcutOptions)>()?)
}

fn ensure_success_response(operation: &str, response_code: u32) -> Result<()> {
    match response_code {
        0 => Ok(()),
        1 => Err(anyhow!("{} canceled by user", operation)),
        2 => Err(anyhow!("{} failed: interaction unavailable", operation)),
        code => Err(anyhow!("{} failed with code {}", operation, code)),
    }
}

fn request_handle_for_token(connection: &Connection, token: &str) -> Result<OwnedObjectPath> {
    let unique_name = connection
        .unique_name()
        .ok_or_else(|| anyhow!("Session bus has no unique name"))?
        .as_str();
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    let request_path = format!(
        "/org/freedesktop/portal/desktop/request/{}/{}",
        sender, token
    );

    OwnedObjectPath::try_from(request_path)
        .map_err(|e| anyhow!("Invalid portal request path: {}", e))
}

fn new_token(prefix: &str) -> String {
    let seq = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", prefix, std::process::id(), seq)
}

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

    fn trigger(self) -> Result<String> {
        let key = keyval_to_shortcuts_key_name(self.keyval)
            .ok_or_else(|| anyhow!("Invalid key name for keyval {}", self.keyval))?;
        let mut parts = Vec::with_capacity(5);

        if self.modifiers & MOD_CTRL != 0 {
            parts.push("CTRL".to_string());
        }
        if self.modifiers & MOD_ALT != 0 {
            parts.push("ALT".to_string());
        }
        if self.modifiers & MOD_SHIFT != 0 {
            parts.push("SHIFT".to_string());
        }
        if self.modifiers & MOD_SUPER != 0 {
            parts.push("LOGO".to_string());
        }

        parts.push(key);
        Ok(parts.join("+"))
    }
}

fn keyval_to_shortcuts_key_name(keyval: u32) -> Option<String> {
    let key: gdk::Key = unsafe { from_glib(keyval) };
    let name = key.name()?.to_string();

    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }

    Some(name)
}

fn normalize_keyval(keyval: u32) -> u32 {
    if (b'A' as u32..=b'Z' as u32).contains(&keyval) {
        keyval + (b'a' - b'A') as u32
    } else {
        keyval
    }
}

fn extract_trigger_description(shortcuts: &PortalShortcutList) -> Option<String> {
    for (shortcut_id, options) in shortcuts {
        if shortcut_id != SHORTCUT_ID {
            continue;
        }
        if let Some(value) = options.get("trigger_description") {
            if let Ok(cloned) = value.try_clone() {
                if let Ok(description) = String::try_from(cloned) {
                    return Some(description);
                }
            }
        }
    }
    None
}

fn normalize_trigger_token(token: &str) -> String {
    let upper = token.trim().to_ascii_uppercase();
    match upper.as_str() {
        "SUPER" | "WIN" | "META" => "LOGO".to_string(),
        "CONTROL" | "PRIMARY" => "CTRL".to_string(),
        _ => upper,
    }
}

fn normalize_trigger(trigger: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();

    for raw in trigger.split('+') {
        let token = normalize_trigger_token(raw);
        match token.as_str() {
            "CTRL" | "ALT" | "SHIFT" | "LOGO" => modifiers.push(token),
            _ => key = token,
        }
    }

    modifiers.sort_unstable();
    if key.is_empty() {
        modifiers.join("+")
    } else if modifiers.is_empty() {
        key
    } else {
        format!("{}+{}", modifiers.join("+"), key)
    }
}

fn triggers_match(actual_trigger: &str, requested_trigger: &str) -> bool {
    let actual = normalize_trigger(actual_trigger);
    let requested = normalize_trigger(requested_trigger);
    !actual.is_empty() && actual == requested
}

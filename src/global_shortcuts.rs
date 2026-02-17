use std::collections::HashMap;
use std::convert::TryFrom;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use glib::translate::from_glib;
use gtk4::gdk;
use log::{error, info, warn};
use tokio::sync::watch;
use zbus::proxy::SignalStream;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use crate::settings::Settings;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";
const HANDY_ENGINE_NAME: &str = "handy";

const SHORTCUT_ID: &str = "push_to_talk";

const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 4;
const MOD_ALT: u32 = 8;
const MOD_SUPER: u32 = 64;

const WATCHED_SETTINGS_KEYS: [&str; 2] = ["push-to-talk-keyval", "push-to-talk-modifiers"];

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

type ShortcutOptions = HashMap<String, OwnedValue>;
type PortalShortcutList = Vec<(String, ShortcutOptions)>;

pub fn start_global_shortcuts_listener() {
    let settings: &'static Settings = Box::leak(Box::new(Settings::new()));
    let initial_config = ShortcutConfig::from_settings(settings);
    let (config_tx, config_rx) = watch::channel(initial_config);

    for key in WATCHED_SETTINGS_KEYS {
        let tx = config_tx.clone();
        settings.connect_changed(Some(key), move |_| {
            let _ = tx.send(ShortcutConfig::from_settings(settings));
        });
    }

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create runtime for global shortcuts: {}", e);
                return;
            }
        };

        runtime.block_on(async move {
            run_listener_loop(config_rx).await;
        });
    });
}

async fn run_listener_loop(mut config_rx: watch::Receiver<ShortcutConfig>) {
    loop {
        let active_config = *config_rx.borrow_and_update();

        match run_shortcut_session(active_config, &mut config_rx).await {
            Ok(()) => {}
            Err(e) => warn!("Global shortcut session ended: {}", e),
        }

        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn run_shortcut_session(
    active_config: ShortcutConfig,
    config_rx: &mut watch::Receiver<ShortcutConfig>,
) -> Result<()> {
    let trigger = active_config
        .trigger()
        .ok_or_else(|| anyhow!("Unsupported push-to-talk shortcut for portal registration"))?;

    let connection = Connection::session().await?;
    let portal_proxy = Proxy::new(&connection, PORTAL_BUS, PORTAL_PATH, SHORTCUTS_IFACE).await?;
    let handy_proxy = Proxy::new(
        &connection,
        HANDY_BUS_NAME,
        HANDY_OBJECT_PATH,
        HANDY_INTERFACE,
    )
    .await?;

    let session_handle = create_session(&portal_proxy, &connection).await?;
    if let Err(e) = bind_shortcut(&portal_proxy, &connection, &session_handle, &trigger).await {
        close_session(&connection, &session_handle).await;
        return Err(e);
    }
    if let Err(e) = list_shortcuts(&portal_proxy, &connection, &session_handle).await {
        warn!("Failed to query registered shortcuts: {}", e);
    }
    info!("Global push-to-talk registered with trigger {}", trigger);

    let mut active_restore_engine: Option<String> = None;
    let mut signal_stream = portal_proxy.receive_all_signals().await?;

    let loop_result = loop {
        tokio::select! {
            changed = config_rx.changed() => {
                if changed.is_err() {
                    break Ok(());
                }
                if *config_rx.borrow_and_update() != active_config {
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
                            on_global_pressed(&handy_proxy, &mut active_restore_engine).await;
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
                            on_global_released(&mut active_restore_engine);
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
        }
    };

    if active_restore_engine.is_some() {
        on_global_released(&mut active_restore_engine);
    }
    close_session(&connection, &session_handle).await;

    loop_result
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
                    return;
                }
            }
        }

        info!("Global shortcut updated by portal");
        return;
    }
}

async fn on_global_pressed(handy_proxy: &Proxy<'_>, active_restore_engine: &mut Option<String>) {
    if active_restore_engine.is_some() {
        return;
    }

    let Some(current_engine) = current_ibus_engine() else {
        warn!("Global PTT press ignored: failed to read current IBus engine");
        return;
    };

    if is_handy_engine(&current_engine) {
        return;
    }

    if !switch_engine(HANDY_ENGINE_NAME) {
        warn!("Global PTT press ignored: failed to switch to Handy source");
        return;
    }

    let start_result: zbus::Result<()> = handy_proxy.call("StartRecording", &()).await;
    if let Err(e) = start_result {
        warn!("Global PTT failed to start recording: {}", e);
        if !switch_engine(&current_engine) {
            warn!(
                "Failed to restore source '{}' after start failure",
                current_engine
            );
        }
        return;
    }

    *active_restore_engine = Some(current_engine);
}

fn on_global_released(active_restore_engine: &mut Option<String>) {
    let Some(restore_engine) = active_restore_engine.take() else {
        return;
    };
    if !switch_engine(&restore_engine) {
        warn!(
            "Failed to restore input source on PTT release: {}",
            restore_engine
        );
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
) -> Result<()> {
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
            log_shortcuts_changed(shortcuts);
        }
    }

    Ok(())
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

fn current_ibus_engine() -> Option<String> {
    let output = Command::new("ibus").arg("engine").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let engine = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if engine.is_empty() {
        None
    } else {
        Some(engine)
    }
}

fn is_handy_engine(engine_name: &str) -> bool {
    engine_name == HANDY_ENGINE_NAME || engine_name.ends_with(":handy")
}

fn switch_engine(engine_name: &str) -> bool {
    if engine_name.is_empty() {
        return false;
    }

    match Command::new("ibus").args(["engine", engine_name]).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            warn!(
                "Failed to switch input source to {}: exit status {}",
                engine_name, status
            );
            false
        }
        Err(e) => {
            warn!("Failed to execute ibus engine {}: {}", engine_name, e);
            false
        }
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

    fn trigger(self) -> Option<String> {
        let key = keyval_to_shortcuts_key_name(self.keyval)?;
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
        Some(parts.join("+"))
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

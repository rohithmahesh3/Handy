use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::{error, info, warn};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use crate::settings::{RecordingMode, Settings};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

const SHORTCUT_ID: &str = "push_to_talk";
const HANDY_ENGINE_NAME: &str = "handy";

const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 4;
const MOD_ALT: u32 = 8;
const MOD_SUPER: u32 = 64;

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn start_global_shortcuts_listener() {
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
            loop {
                if Settings::new().recording_mode() != RecordingMode::PushToTalk {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }

                match run_shortcut_session().await {
                    Ok(()) => {}
                    Err(e) => warn!("Global shortcut session ended: {}", e),
                }

                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    });
}

async fn run_shortcut_session() -> Result<()> {
    let settings = Settings::new();
    let trigger = shortcut_trigger(&settings)
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
    bind_shortcut(&portal_proxy, &connection, &session_handle, &trigger).await?;
    info!("Global push-to-talk registered with trigger {}", trigger);

    let mut active_restore_engine: Option<String> = None;
    let mut signal_stream = portal_proxy.receive_all_signals().await?;

    while let Some(signal_msg) = signal_stream.next().await {
        let header = signal_msg.header();
        let Some(member) = header.member() else {
            continue;
        };

        if member.as_str() == "Activated" {
            let (handle, shortcut_id, _timestamp, _options): (
                OwnedObjectPath,
                String,
                u32,
                HashMap<String, OwnedValue>,
            ) = signal_msg.body().deserialize()?;
            if handle == session_handle && shortcut_id == SHORTCUT_ID {
                on_global_pressed(&handy_proxy, &mut active_restore_engine).await;
            }
        } else if member.as_str() == "Deactivated" {
            let (handle, shortcut_id, _timestamp, _options): (
                OwnedObjectPath,
                String,
                u32,
                HashMap<String, OwnedValue>,
            ) = signal_msg.body().deserialize()?;
            if handle == session_handle && shortcut_id == SHORTCUT_ID {
                on_global_released(&mut active_restore_engine);
            }
        }
    }

    Err(anyhow!("Global shortcut signal stream closed"))
}

async fn on_global_pressed(handy_proxy: &Proxy<'_>, active_restore_engine: &mut Option<String>) {
    if Settings::new().recording_mode() != RecordingMode::PushToTalk {
        return;
    }
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

    let start_result: zbus::Result<()> = handy_proxy.call("StartRecording", &()).await;
    if let Err(e) = start_result {
        warn!("Global PTT failed to start recording: {}", e);
        return;
    }

    *active_restore_engine = Some(current_engine);
    switch_engine_async(HANDY_ENGINE_NAME.to_string());
}

fn on_global_released(active_restore_engine: &mut Option<String>) {
    let Some(restore_engine) = active_restore_engine.take() else {
        return;
    };
    switch_engine_async(restore_engine);
}

async fn create_session(
    portal_proxy: &Proxy<'_>,
    connection: &Connection,
) -> Result<OwnedObjectPath> {
    let mut options: HashMap<String, OwnedValue> = HashMap::new();
    options.insert(
        "handle_token".to_string(),
        Value::from(new_token("handy_gs_create")).try_into()?,
    );
    options.insert(
        "session_handle_token".to_string(),
        Value::from(new_token("handy_gs_session")).try_into()?,
    );

    let request_handle: OwnedObjectPath = portal_proxy.call("CreateSession", &(options)).await?;
    let (response_code, mut response_data) =
        await_request_response(connection, request_handle).await?;

    if response_code != 0 {
        return Err(anyhow!(
            "GlobalShortcuts CreateSession failed with code {}",
            response_code
        ));
    }

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
    let mut shortcut_options: HashMap<String, OwnedValue> = HashMap::new();
    shortcut_options.insert(
        "description".to_string(),
        Value::from("Handy push-to-talk").try_into()?,
    );
    shortcut_options.insert(
        "preferred_trigger".to_string(),
        Value::from(trigger).try_into()?,
    );
    let shortcuts = vec![(SHORTCUT_ID.to_string(), shortcut_options)];

    let mut bind_options: HashMap<String, OwnedValue> = HashMap::new();
    bind_options.insert(
        "handle_token".to_string(),
        Value::from(new_token("handy_gs_bind")).try_into()?,
    );

    let request_handle: OwnedObjectPath = portal_proxy
        .call(
            "BindShortcuts",
            &(session_handle, shortcuts, String::new(), bind_options),
        )
        .await?;
    let (response_code, _) = await_request_response(connection, request_handle).await?;

    if response_code != 0 {
        return Err(anyhow!(
            "GlobalShortcuts BindShortcuts failed with code {}",
            response_code
        ));
    }

    Ok(())
}

async fn await_request_response(
    connection: &Connection,
    request_handle: OwnedObjectPath,
) -> Result<(u32, HashMap<String, OwnedValue>)> {
    let request_proxy = Proxy::new(
        connection,
        PORTAL_BUS,
        request_handle.as_str(),
        REQUEST_IFACE,
    )
    .await?;
    let mut response_stream = request_proxy.receive_signal("Response").await?;
    let response_msg = response_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("Portal request response stream ended"))?;
    let response = response_msg
        .body()
        .deserialize::<(u32, HashMap<String, OwnedValue>)>()?;
    Ok(response)
}

fn new_token(prefix: &str) -> String {
    let seq = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", prefix, std::process::id(), seq)
}

fn shortcut_trigger(settings: &Settings) -> Option<String> {
    let key = keyval_to_name(settings.push_to_talk_keyval())?;
    let mut trigger = String::new();
    let modifiers = settings.push_to_talk_modifiers();

    if modifiers & MOD_CTRL != 0 {
        trigger.push_str("<Ctrl>");
    }
    if modifiers & MOD_ALT != 0 {
        trigger.push_str("<Alt>");
    }
    if modifiers & MOD_SHIFT != 0 {
        trigger.push_str("<Shift>");
    }
    if modifiers & MOD_SUPER != 0 {
        trigger.push_str("<Super>");
    }

    trigger.push_str(&key);
    Some(trigger)
}

fn keyval_to_name(keyval: u32) -> Option<String> {
    if keyval == 32 {
        return Some("space".to_string());
    }

    char::from_u32(keyval).and_then(|ch| {
        if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase().to_string())
        } else {
            None
        }
    })
}

fn current_ibus_engine() -> Option<String> {
    let output = std::process::Command::new("ibus")
        .arg("engine")
        .output()
        .ok()?;
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

fn switch_engine_async(engine_name: String) {
    if engine_name.is_empty() || is_handy_engine(&engine_name) {
        return;
    }

    std::thread::spawn(move || {
        match std::process::Command::new("ibus")
            .args(["engine", &engine_name])
            .status()
        {
            Ok(status) if status.success() => {
                info!("Switched input source to {}", engine_name);
            }
            Ok(status) => {
                warn!(
                    "Failed to switch input source to {}: exit status {}",
                    engine_name, status
                );
            }
            Err(e) => {
                warn!("Failed to execute ibus engine {}: {}", engine_name, e);
            }
        }
    });
}

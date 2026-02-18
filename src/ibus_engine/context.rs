use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, error, info, warn};
use notify_rust::Notification;
use zbus::blocking::Connection;

use ibus_sys::keys::IBUS_KEY_Escape;
use ibus_sys::modifiers::IBUS_RELEASE_MASK;
use ibus_sys::{g_object_unref, gboolean, gpointer, guint, IBusEngine, TRUE};

use super::ibus_api::{current_ibus_engine, is_handy_engine, switch_engine_async};
use crate::settings::Settings;
use crate::utils::launch::open_handy_ui;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";
const LIVE_PARTIAL_POLL_MS: u64 = 220;
const PENDING_COMMIT_POLL_MS: u64 = 100;
const STOP_RECORDING_TIMEOUT_MS: u64 = 20_000;

pub struct HandyContext {
    connection: Option<Connection>,
    settings: Settings,
    is_recording: bool,
    is_focused: bool,
    is_enabled: bool,
    notification_shown: bool,
    last_non_handy_engine: Option<String>,
    active_session_id: Option<u64>,
    live_partial_cancel: Option<Arc<AtomicBool>>,
    pending_commit_cancel: Option<Arc<AtomicBool>>,
}

impl HandyContext {
    pub fn new() -> Self {
        let mut context = Self {
            connection: None,
            settings: Settings::new(),
            is_recording: false,
            is_focused: false,
            is_enabled: false,
            notification_shown: false,
            last_non_handy_engine: None,
            active_session_id: None,
            live_partial_cancel: None,
            pending_commit_cancel: None,
        };
        context.refresh_last_non_handy_engine();
        context
    }

    fn try_connect(&mut self) -> bool {
        if self.connection.is_some() {
            return true;
        }

        match Connection::session() {
            Ok(conn) => {
                self.connection = Some(conn);
                info!("Connected to D-Bus session bus");
                true
            }
            Err(e) => {
                error!("Failed to connect to D-Bus: {}", e);
                false
            }
        }
    }

    pub fn focus_in(&mut self, _engine: *mut IBusEngine) {
        debug!("Focus in");
        self.is_focused = true;

        if self.connection.is_none() {
            let _ = self.try_connect();
        }
    }

    pub fn focus_out(&mut self, engine: *mut IBusEngine) {
        debug!("Focus out");
        self.is_focused = false;

        if self.is_recording && !self.is_enabled {
            self.stop_and_commit(engine, None, false);
        }
    }

    pub fn reset(&mut self, _engine: *mut IBusEngine) {
        debug!("Reset");
    }

    pub fn enable(&mut self, engine: *mut IBusEngine) {
        debug!("Engine enabled");
        self.is_enabled = true;

        if self.connection.is_none() && !self.try_connect() {
            return;
        }

        self.ensure_pending_commit_listener(engine);
        self.ensure_live_partial_listener(engine);

        if !self.notification_shown {
            if let Some(conn) = &self.connection {
                match conn.call_method(
                    Some(HANDY_BUS_NAME),
                    HANDY_OBJECT_PATH,
                    Some(HANDY_INTERFACE),
                    "GetState",
                    &(),
                ) {
                    Ok(reply) => {
                        if let Ok((_, has_model)) = reply.body().deserialize::<(bool, bool)>() {
                            if !has_model {
                                self.show_model_notification();
                                self.notification_shown = true;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get state from daemon: {}", e);
                        self.show_service_notification();
                        self.notification_shown = true;
                    }
                }
            }
        }
    }

    fn ensure_live_partial_listener(&mut self, engine: *mut IBusEngine) {
        if !self.settings.live_partial_enabled() {
            self.stop_live_partial_listener();
            return;
        }

        if self.live_partial_cancel.is_some() {
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.live_partial_cancel = Some(cancel.clone());

        unsafe {
            ibus_sys::g_object_ref(engine as gpointer);
        }

        let engine_addr = engine as usize;
        std::thread::spawn(move || {
            let conn = match Connection::session() {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to create partial listener DBus connection: {}", e);
                    glib::MainContext::default().invoke(move || unsafe {
                        g_object_unref(engine_addr as gpointer);
                    });
                    return;
                }
            };

            let mut last_sequence: u64 = 0;
            let mut preedit_visible = false;

            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(LIVE_PARTIAL_POLL_MS));
                if cancel.load(Ordering::SeqCst) {
                    break;
                }

                let reply = conn.call_method(
                    Some(HANDY_BUS_NAME),
                    HANDY_OBJECT_PATH,
                    Some(HANDY_INTERFACE),
                    "GetLatestPartial",
                    &(),
                );

                let Ok(reply) = reply else {
                    continue;
                };

                let Ok((_session_id, sequence_id, text)) =
                    reply.body().deserialize::<(u64, u64, String)>()
                else {
                    continue;
                };

                if sequence_id == 0 {
                    last_sequence = 0;
                    if preedit_visible {
                        preedit_visible = false;
                        glib::MainContext::default().invoke(move || {
                            let engine_ptr = engine_addr as *mut IBusEngine;
                            clear_preedit_text(engine_ptr);
                        });
                    }
                    continue;
                }

                if sequence_id <= last_sequence {
                    continue;
                }
                last_sequence = sequence_id;

                let partial = text.trim().to_string();
                if partial.is_empty() {
                    continue;
                }

                preedit_visible = true;
                glib::MainContext::default().invoke(move || {
                    let engine_ptr = engine_addr as *mut IBusEngine;
                    update_preedit_text_to_engine(engine_ptr, &partial);
                });
            }

            glib::MainContext::default().invoke(move || {
                let engine_ptr = engine_addr as *mut IBusEngine;
                clear_preedit_text(engine_ptr);
                unsafe {
                    g_object_unref(engine_ptr as gpointer);
                }
            });
        });
    }

    fn stop_live_partial_listener(&mut self) {
        if let Some(cancel) = self.live_partial_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn ensure_pending_commit_listener(&mut self, engine: *mut IBusEngine) {
        if self.pending_commit_cancel.is_some() {
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_commit_cancel = Some(cancel.clone());

        unsafe {
            ibus_sys::g_object_ref(engine as gpointer);
        }

        let engine_addr = engine as usize;
        std::thread::spawn(move || {
            let conn = match Connection::session() {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to create pending commit DBus connection: {}", e);
                    glib::MainContext::default().invoke(move || unsafe {
                        g_object_unref(engine_addr as gpointer);
                    });
                    return;
                }
            };

            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(PENDING_COMMIT_POLL_MS));
                if cancel.load(Ordering::SeqCst) {
                    break;
                }

                let reply = conn.call_method(
                    Some(HANDY_BUS_NAME),
                    HANDY_OBJECT_PATH,
                    Some(HANDY_INTERFACE),
                    "TakePendingCommit",
                    &(),
                );

                let Ok(reply) = reply else {
                    continue;
                };
                let Ok((session_id, text)) = reply.body().deserialize::<(u64, String)>() else {
                    continue;
                };

                let final_text = text.trim().to_string();
                if session_id == 0 || final_text.is_empty() {
                    continue;
                }

                glib::MainContext::default().invoke(move || {
                    let engine_ptr = engine_addr as *mut IBusEngine;
                    clear_preedit_text(engine_ptr);
                    info!(
                        "Committed pending transcription from session {} while engine stayed active",
                        session_id
                    );
                    commit_text_to_engine(engine_ptr, &final_text);
                });
            }

            glib::MainContext::default().invoke(move || unsafe {
                g_object_unref(engine_addr as gpointer);
            });
        });
    }

    fn stop_pending_commit_listener(&mut self) {
        if let Some(cancel) = self.pending_commit_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn show_model_notification(&self) {
        debug!("Showing model notification");

        std::thread::spawn(|| {
            let notification = Notification::new()
                .summary("Handy Speech-to-Text")
                .body("No speech model configured. Click to open preferences.")
                .timeout(notify_rust::Timeout::Never)
                .action("default", "Open Preferences")
                .show();

            match notification {
                Ok(handle) => {
                    handle.wait_for_action(|action| {
                        if action == "default" || action == "clicked" {
                            info!("Notification clicked, opening Handy GUI");
                            if let Err(e) = open_handy_ui(None) {
                                error!("Failed to spawn handy: {}", e);
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to show notification: {}", e);
                }
            }
        });
    }

    fn show_service_notification(&self) {
        debug!("Showing service notification");

        std::thread::spawn(|| {
            let notification = Notification::new()
                .summary("Handy Speech-to-Text")
                .body("Handy service is not running. Click to open preferences and start it.")
                .timeout(notify_rust::Timeout::Never)
                .action("default", "Open Preferences")
                .show();

            match notification {
                Ok(handle) => {
                    handle.wait_for_action(|action| {
                        if action == "default" || action == "clicked" {
                            info!("Service notification clicked, opening Handy GUI");
                            if let Err(e) = open_handy_ui(None) {
                                error!("Failed to spawn handy: {}", e);
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to show notification: {}", e);
                }
            }
        });
    }

    pub fn disable(&mut self, engine: *mut IBusEngine) {
        debug!("Engine disabled");
        self.stop_pending_commit_listener();
        self.stop_live_partial_listener();
        self.commit_pending_transcription(engine);
        self.is_enabled = false;
        self.is_focused = false;
        self.active_session_id = None;
        self.notification_shown = false;
        self.refresh_last_non_handy_engine();
    }

    pub fn process_key_event(
        &mut self,
        _engine: *mut IBusEngine,
        keyval: guint,
        _keycode: guint,
        modifiers: guint,
    ) -> gboolean {
        let is_release = modifiers & IBUS_RELEASE_MASK != 0;
        if !is_release && keyval == IBUS_KEY_Escape && self.is_recording {
            debug!("Escape pressed while recording, cancelling recording");
            self.cancel_recording();
            return TRUE;
        }

        0
    }

    fn stop_and_commit(
        &mut self,
        engine: *mut IBusEngine,
        restore_engine: Option<String>,
        force: bool,
    ) {
        if !self.is_recording && !force {
            return;
        }

        if self.is_recording {
            info!("Stopping recording");
        } else {
            debug!("Checking for active recording before commit");
        }
        self.is_recording = false;
        let session_id = self.active_session_id.take();

        let Some(conn) = self.connection.as_ref().cloned() else {
            return;
        };

        unsafe {
            ibus_sys::g_object_ref(engine as gpointer);
        }

        let engine_addr = engine as usize;
        std::thread::spawn(move || {
            let mut final_text = match call_stop_recording_with_timeout(
                &conn,
                session_id,
                Duration::from_millis(STOP_RECORDING_TIMEOUT_MS),
            ) {
                Ok(text) => text,
                Err(err) => {
                    warn!("StopRecording failed: {}", err);
                    let fallback = latest_partial_for_session(&conn, session_id);
                    if let Some(partial) = fallback {
                        info!("Using partial fallback text after stop failure");
                        partial
                    } else {
                        let _ = call_cancel_recording(&conn);
                        String::new()
                    }
                }
            };

            if final_text.trim().is_empty() {
                final_text.clear();
            }

            glib::MainContext::default().invoke(move || {
                let engine_ptr = engine_addr as *mut IBusEngine;
                clear_preedit_text(engine_ptr);
                if !final_text.is_empty() {
                    commit_text_to_engine(engine_ptr, &final_text);
                }
                unsafe {
                    g_object_unref(engine_ptr as gpointer);
                }
                if let Some(target_engine) = restore_engine {
                    switch_engine_async(target_engine);
                }
            });
        });
    }

    fn commit_pending_transcription(&mut self, engine: *mut IBusEngine) {
        if self.connection.is_none() && !self.try_connect() {
            clear_preedit_text(engine);
            return;
        }

        let Some(conn) = self.connection.as_ref() else {
            clear_preedit_text(engine);
            return;
        };

        let reply = conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "TakePendingCommit",
            &(),
        );

        let (session_id, text) = match reply {
            Ok(reply) => reply
                .body()
                .deserialize::<(u64, String)>()
                .unwrap_or((0, String::new())),
            Err(e) => {
                debug!("TakePendingCommit unavailable on disable: {}", e);
                (0, String::new())
            }
        };

        clear_preedit_text(engine);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            debug!("No pending commit payload found on engine disable");
            return;
        }

        info!(
            "Committing pending transcription from session {} ({} chars)",
            session_id,
            trimmed.chars().count()
        );
        commit_text_to_engine(engine, trimmed);
    }

    fn cancel_recording(&mut self) {
        if !self.is_recording {
            return;
        }

        info!("Cancelling recording");
        self.is_recording = false;
        self.active_session_id = None;

        let Some(conn) = &self.connection else {
            return;
        };

        if let Err(e) = conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "CancelRecording",
            &(),
        ) {
            error!("Failed to cancel recording: {}", e);
        }
    }

    fn refresh_last_non_handy_engine(&mut self) {
        if let Some(engine_name) = current_ibus_engine() {
            if !is_handy_engine(&engine_name) {
                self.last_non_handy_engine = Some(engine_name);
            }
        }
    }
}

fn call_stop_recording(conn: &Connection, session_id: Option<u64>) -> Result<String, String> {
    let reply = match session_id {
        Some(id) => conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StopRecordingSession",
            &(id,),
        ),
        None => conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StopRecording",
            &(),
        ),
    }
    .map_err(|e| format!("D-Bus stop call failed: {}", e))?;

    reply
        .body()
        .deserialize::<String>()
        .map_err(|e| format!("Failed to decode stop response: {}", e))
}

fn call_stop_recording_with_timeout(
    conn: &Connection,
    session_id: Option<u64>,
    timeout: Duration,
) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let conn = conn.clone();
    std::thread::spawn(move || {
        let _ = tx.send(call_stop_recording(&conn, session_id));
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(format!(
            "Timed out waiting for stop after {} ms",
            timeout.as_millis()
        )),
        Err(RecvTimeoutError::Disconnected) => Err("Stop worker disconnected".to_string()),
    }
}

fn latest_partial_for_session(
    conn: &Connection,
    expected_session_id: Option<u64>,
) -> Option<String> {
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "GetLatestPartial",
            &(),
        )
        .ok()?;
    let (session_id, sequence_id, text) = reply.body().deserialize::<(u64, u64, String)>().ok()?;
    if sequence_id == 0 {
        return None;
    }

    if let Some(expected) = expected_session_id {
        if session_id != expected {
            return None;
        }
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn call_cancel_recording(conn: &Connection) -> Result<(), String> {
    conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        "CancelRecording",
        &(),
    )
    .map_err(|e| format!("CancelRecording call failed: {}", e))?;
    Ok(())
}

fn update_preedit_text_to_engine(engine: *mut IBusEngine, text: &str) {
    let c_text = match CString::new(text) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create preedit CString: {}", e);
            return;
        }
    };

    unsafe {
        let ibus_text = ibus_sys::ibus_text_new_from_string(c_text.as_ptr());
        if !ibus_text.is_null() {
            ibus_sys::ibus_engine_update_preedit_text(
                engine,
                ibus_text,
                text.chars().count() as u32,
                TRUE,
            );
            ibus_sys::ibus_engine_show_preedit_text(engine);
            g_object_unref(ibus_text as gpointer);
        }
    }
}

fn clear_preedit_text(engine: *mut IBusEngine) {
    unsafe {
        ibus_sys::ibus_engine_hide_preedit_text(engine);
    }
}

fn commit_text_to_engine(engine: *mut IBusEngine, text: &str) {
    let preview: String = text.chars().take(50).collect();
    info!("Committing text: {}...", preview);

    let c_text = match CString::new(text) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create CString: {}", e);
            return;
        }
    };

    unsafe {
        let ibus_text = ibus_sys::ibus_text_new_from_string(c_text.as_ptr());
        if !ibus_text.is_null() {
            ibus_sys::ibus_engine_commit_text(engine, ibus_text);
            g_object_unref(ibus_text as gpointer);
        }
    }
}

pub type SharedContext = Arc<Mutex<HandyContext>>;

#[allow(clippy::arc_with_non_send_sync)]
pub fn create_context() -> SharedContext {
    Arc::new(Mutex::new(HandyContext::new()))
}

unsafe extern "C" fn process_key_event_callback(
    context: *mut c_void,
    engine: *mut IBusEngine,
    keyval: guint,
    keycode: guint,
    modifiers: guint,
) -> gboolean {
    if context.is_null() || engine.is_null() {
        return 0;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.process_key_event(engine, keyval, keycode, modifiers)
    } else {
        0
    }
}

unsafe extern "C" fn focus_in_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.focus_in(engine);
    }
}

unsafe extern "C" fn focus_out_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.focus_out(engine);
    }
}

unsafe extern "C" fn reset_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.reset(engine);
    }
}

unsafe extern "C" fn enable_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.enable(engine);
    }
}

unsafe extern "C" fn disable_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const Mutex<HandyContext>);
    if let Ok(mut ctx) = context.lock() {
        ctx.disable(engine);
    }
}

extern "C" {
    fn ibus_handy_set_callback(
        ctx: *mut c_void,
        key_event_cb: unsafe extern "C" fn(
            *mut c_void,
            *mut IBusEngine,
            guint,
            guint,
            guint,
        ) -> gboolean,
        focus_in_cb: unsafe extern "C" fn(*mut c_void, *mut IBusEngine),
        focus_out_cb: unsafe extern "C" fn(*mut c_void, *mut IBusEngine),
        reset_cb: unsafe extern "C" fn(*mut c_void, *mut IBusEngine),
        enable_cb: unsafe extern "C" fn(*mut c_void, *mut IBusEngine),
        disable_cb: unsafe extern "C" fn(*mut c_void, *mut IBusEngine),
    );
}

pub fn init(context: &SharedContext) {
    unsafe {
        ibus_handy_set_callback(
            Arc::as_ptr(context) as *mut c_void,
            process_key_event_callback,
            focus_in_callback,
            focus_out_callback,
            reset_callback,
            enable_callback,
            disable_callback,
        );
    }
}

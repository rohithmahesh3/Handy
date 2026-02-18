use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, error, info, warn};
use notify_rust::Notification;
use zbus::blocking::Connection;

use crate::utils::launch::open_handy_ui;
use ibus_sys::{g_object_unref, gboolean, gpointer, guint, IBusEngine};

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";
const PENDING_COMMIT_POLL_MS: u64 = 60;
const PENDING_COMMIT_FAILURE_RECONNECT_THRESHOLD: u64 = 5;

pub struct HandyContext {
    connection: Option<Connection>,
    is_focused: bool,
    is_enabled: bool,
    notification_shown: bool,
    pending_commit_cancel: Option<Arc<AtomicBool>>,
    pending_commit_engine_addr: Option<usize>,
}

impl HandyContext {
    pub fn new() -> Self {
        Self {
            connection: None,
            is_focused: false,
            is_enabled: false,
            notification_shown: false,
            pending_commit_cancel: None,
            pending_commit_engine_addr: None,
        }
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
        self.set_focused_engine_state(_engine, true);
    }

    pub fn focus_out(&mut self, engine: *mut IBusEngine) {
        debug!("Focus out");
        self.is_focused = false;
        self.set_focused_engine_state(engine, false);
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

        self.set_focused_engine_state(engine, self.is_focused);
        self.ensure_pending_commit_listener(engine);

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

    fn ensure_pending_commit_listener(&mut self, engine: *mut IBusEngine) {
        let engine_addr = engine as usize;
        if self.pending_commit_cancel.is_some() {
            if self.pending_commit_engine_addr == Some(engine_addr) {
                return;
            }
            self.stop_pending_commit_listener();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_commit_cancel = Some(cancel.clone());
        self.pending_commit_engine_addr = Some(engine_addr);

        unsafe {
            ibus_sys::g_object_ref(engine as gpointer);
        }

        std::thread::spawn(move || {
            let mut conn = match Connection::session() {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to create pending commit DBus connection: {}", e);
                    glib::MainContext::default().invoke(move || unsafe {
                        g_object_unref(engine_addr as gpointer);
                    });
                    return;
                }
            };
            let mut failure_streak: u64 = 0;

            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(PENDING_COMMIT_POLL_MS));
                if cancel.load(Ordering::SeqCst) {
                    break;
                }

                let engine_id = engine_addr as u64;
                let reply = conn.call_method(
                    Some(HANDY_BUS_NAME),
                    HANDY_OBJECT_PATH,
                    Some(HANDY_INTERFACE),
                    "TakePendingCommitForEngine",
                    &(engine_id,),
                );

                let reply = match reply {
                    Ok(reply) => reply,
                    Err(e) => {
                        failure_streak = failure_streak.saturating_add(1);
                        if failure_streak == 1 || failure_streak.is_multiple_of(10) {
                            warn!(
                                "TakePendingCommitForEngine call failed (streak={}): {}",
                                failure_streak, e
                            );
                        }
                        if failure_streak >= PENDING_COMMIT_FAILURE_RECONNECT_THRESHOLD {
                            match Connection::session() {
                                Ok(new_conn) => {
                                    warn!(
                                        "Reconnected pending commit listener DBus session after {} failures",
                                        failure_streak
                                    );
                                    conn = new_conn;
                                    failure_streak = 0;
                                }
                                Err(reconnect_err) => {
                                    warn!(
                                        "Pending commit listener reconnect failed after {} errors: {}",
                                        failure_streak, reconnect_err
                                    );
                                }
                            }
                        }
                        continue;
                    }
                };
                if failure_streak > 0 {
                    info!(
                        "Pending commit listener recovered after {} consecutive errors",
                        failure_streak
                    );
                    failure_streak = 0;
                }
                let Ok((session_id, text)) = reply.body().deserialize::<(u64, String)>() else {
                    warn!("TakePendingCommitForEngine returned an invalid payload");
                    continue;
                };

                let final_text = text.trim().to_string();
                if session_id == 0 || final_text.is_empty() {
                    continue;
                }
                let commit_success_ms = now_millis();

                glib::MainContext::default().invoke(move || {
                    let engine_ptr = engine_addr as *mut IBusEngine;
                    info!(
                        "Committed pending transcription from session {} while engine stayed active (last_success_ms={})",
                        session_id, commit_success_ms
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
        self.pending_commit_engine_addr = None;
    }

    fn set_focused_engine_state(&mut self, engine: *mut IBusEngine, focused: bool) {
        if engine.is_null() {
            return;
        }
        if self.connection.is_none() && !self.try_connect() {
            return;
        }
        let Some(conn) = self.connection.clone() else {
            return;
        };
        let engine_id = engine as usize as u64;
        if let Err(e) = conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "SetFocusedEngine",
            &(engine_id, focused),
        ) {
            warn!(
                "SetFocusedEngine(engine_id={}, focused={}) failed: {}",
                engine_id, focused, e
            );
            self.connection = None;
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
        self.set_focused_engine_state(engine, false);
        self.stop_pending_commit_listener();
        self.commit_pending_transcription(engine);
        self.is_enabled = false;
        self.is_focused = false;
        self.notification_shown = false;
    }

    pub fn process_key_event(
        &mut self,
        _engine: *mut IBusEngine,
        _keyval: guint,
        _keycode: guint,
        _modifiers: guint,
    ) -> gboolean {
        0
    }

    fn commit_pending_transcription(&mut self, engine: *mut IBusEngine) {
        if self.connection.is_none() && !self.try_connect() {
            return;
        }

        let Some(conn) = self.connection.as_ref() else {
            return;
        };

        let reply = conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "TakePendingCommitForEngine",
            &(engine as usize as u64,),
        );

        let (session_id, text) = match reply {
            Ok(reply) => reply
                .body()
                .deserialize::<(u64, String)>()
                .unwrap_or((0, String::new())),
            Err(e) => {
                debug!("TakePendingCommitForEngine unavailable on disable: {}", e);
                (0, String::new())
            }
        };

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
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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

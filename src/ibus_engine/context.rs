use std::ffi::{c_void, CString};
use std::sync::{Arc, Mutex};

use log::{debug, error, info, warn};
use notify_rust::Notification;
use zbus::blocking::Connection;

use ibus_sys::keys::IBUS_KEY_Escape;
use ibus_sys::modifiers::IBUS_RELEASE_MASK;
use ibus_sys::{g_object_unref, gboolean, gpointer, guint, IBusEngine, TRUE};

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

pub struct HandyContext {
    connection: Option<Connection>,
    is_recording: bool,
    is_focused: bool,
    is_enabled: bool,
    notification_shown: bool,
}

impl HandyContext {
    pub fn new() -> Self {
        Self {
            connection: None,
            is_recording: false,
            is_focused: false,
            is_enabled: false,
            notification_shown: false,
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

    pub fn focus_in(&mut self, engine: *mut IBusEngine) {
        debug!("Focus in");
        self.is_focused = true;

        if self.connection.is_none() && !self.try_connect() {
            return;
        }

        if self.is_enabled && !self.is_recording {
            self.start_recording(engine);
        }
    }

    pub fn focus_out(&mut self, engine: *mut IBusEngine) {
        debug!("Focus out");
        self.is_focused = false;

        if self.is_recording {
            self.stop_and_commit(engine);
        }
    }

    pub fn reset(&mut self, _engine: *mut IBusEngine) {
        debug!("Reset");
        if self.is_recording {
            self.cancel_recording();
        }
    }

    pub fn enable(&mut self, engine: *mut IBusEngine) {
        debug!("Engine enabled");
        self.is_enabled = true;

        if self.connection.is_none() && !self.try_connect() {
            return;
        }

        // Check if model is loaded, show notification if not
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
                        if let Ok((_, is_model_loaded)) = reply.body().deserialize::<(bool, bool)>()
                        {
                            if !is_model_loaded {
                                self.show_model_notification();
                                self.notification_shown = true;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get state: {}", e);
                    }
                }
            }
        }

        if self.is_focused && !self.is_recording {
            self.start_recording(engine);
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
                            if let Err(e) = std::process::Command::new("/usr/bin/handy").spawn() {
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

    pub fn disable(&mut self, _engine: *mut IBusEngine) {
        debug!("Engine disabled");
        self.is_enabled = false;
        self.is_focused = false;
        self.notification_shown = false; // Reset so notification shows again on next enable
        if self.is_recording {
            self.cancel_recording();
        }
    }

    pub fn process_key_event(
        &mut self,
        _engine: *mut IBusEngine,
        keyval: guint,
        _keycode: guint,
        modifiers: guint,
    ) -> gboolean {
        if modifiers & IBUS_RELEASE_MASK != 0 {
            return 0;
        }

        if keyval == IBUS_KEY_Escape && self.is_recording {
            debug!("Escape pressed, cancelling recording");
            self.cancel_recording();
            return TRUE;
        }

        0
    }

    fn start_recording(&mut self, _engine: *mut IBusEngine) {
        if self.is_recording {
            return;
        }

        let Some(conn) = &self.connection else {
            warn!("Cannot start recording: not connected to D-Bus");
            return;
        };

        info!("Starting recording");
        match conn.call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StartRecording",
            &(),
        ) {
            Ok(_) => {
                self.is_recording = true;
            }
            Err(e) => {
                error!("Failed to start recording: {}", e);
            }
        }
    }

    fn stop_and_commit(&mut self, engine: *mut IBusEngine) {
        if !self.is_recording {
            return;
        }

        info!("Stopping recording");
        self.is_recording = false;

        let Some(conn) = self.connection.as_ref().cloned() else {
            return;
        };

        // Prevent use-after-free: ref the GObject so IBus can't destroy it
        // while our background thread is working.
        unsafe {
            ibus_sys::g_object_ref(engine as gpointer);
        }

        // Stop/transcribe can take seconds; do this off the IBus callback thread.
        let engine_addr = engine as usize;
        std::thread::spawn(move || {
            let text = match conn.call_method(
                Some(HANDY_BUS_NAME),
                HANDY_OBJECT_PATH,
                Some(HANDY_INTERFACE),
                "StopRecording",
                &(),
            ) {
                Ok(reply) => match reply.body().deserialize::<String>() {
                    Ok(text) => text,
                    Err(e) => {
                        error!("Failed to deserialize transcription response: {}", e);
                        String::new()
                    }
                },
                Err(e) => {
                    error!("Failed to stop recording: {}", e);
                    String::new()
                }
            };

            if text.is_empty() {
                unsafe {
                    g_object_unref(engine_addr as gpointer);
                }
                return;
            }

            // Commit back on the GLib/IBus main context.
            glib::MainContext::default().invoke(move || {
                let engine_ptr = engine_addr as *mut IBusEngine;
                commit_text_to_engine(engine_ptr, &text);
                unsafe {
                    g_object_unref(engine_ptr as gpointer);
                }
            });
        });
    }

    fn cancel_recording(&mut self) {
        if !self.is_recording {
            return;
        }

        info!("Cancelling recording");
        self.is_recording = false;

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
    let context = &*(context as *const SharedContext);
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
    let context = &*(context as *const SharedContext);
    if let Ok(mut ctx) = context.lock() {
        ctx.focus_in(engine);
    }
}

unsafe extern "C" fn focus_out_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const SharedContext);
    if let Ok(mut ctx) = context.lock() {
        ctx.focus_out(engine);
    }
}

unsafe extern "C" fn reset_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const SharedContext);
    if let Ok(mut ctx) = context.lock() {
        ctx.reset(engine);
    }
}

unsafe extern "C" fn enable_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const SharedContext);
    if let Ok(mut ctx) = context.lock() {
        ctx.enable(engine);
    }
}

unsafe extern "C" fn disable_callback(context: *mut c_void, engine: *mut IBusEngine) {
    if context.is_null() || engine.is_null() {
        return;
    }
    let context = &*(context as *const SharedContext);
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

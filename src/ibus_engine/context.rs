use std::ffi::{c_void, CString};
use std::sync::{Arc, Mutex};

use log::{debug, error, info, warn};
use notify_rust::Notification;
use zbus::blocking::Connection;

use ibus_sys::keys::IBUS_KEY_Escape;
use ibus_sys::modifiers::IBUS_RELEASE_MASK;
use ibus_sys::{g_object_unref, gboolean, gpointer, guint, IBusEngine, TRUE};

use super::ibus_api::{current_ibus_engine, is_handy_engine, switch_engine_async};
use crate::settings::Settings;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";
pub struct HandyContext {
    connection: Option<Connection>,
    settings: Settings,
    is_recording: bool,
    is_focused: bool,
    is_enabled: bool,
    notification_shown: bool,
    ptt_pressed: bool,
    last_non_handy_engine: Option<String>,
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
            ptt_pressed: false,
            last_non_handy_engine: None,
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

    pub fn enable(&mut self, _engine: *mut IBusEngine) {
        debug!("Engine enabled");
        self.is_enabled = true;

        if self.connection.is_none() && !self.try_connect() {
            return;
        }

        // Check if model is selected, show notification if not
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
                        // Daemon might not be running — show service notification
                        self.show_service_notification();
                        self.notification_shown = true;
                    }
                }
            }
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

    pub fn disable(&mut self, engine: *mut IBusEngine) {
        debug!("Engine disabled");
        self.stop_and_commit(engine, None, true);
        self.is_enabled = false;
        self.is_focused = false;
        self.ptt_pressed = false;
        self.notification_shown = false; // Reset so notification shows again on next enable
        self.refresh_last_non_handy_engine();
    }

    pub fn process_key_event(
        &mut self,
        engine: *mut IBusEngine,
        keyval: guint,
        _keycode: guint,
        modifiers: guint,
    ) -> gboolean {
        let is_release = modifiers & IBUS_RELEASE_MASK != 0;
        self.process_push_to_talk_key_event(engine, keyval, modifiers, is_release)
    }

    fn process_push_to_talk_key_event(
        &mut self,
        engine: *mut IBusEngine,
        keyval: guint,
        modifiers: guint,
        is_release: bool,
    ) -> gboolean {
        let ptt_keyval = self.settings.push_to_talk_keyval();
        let ptt_modifiers = self.settings.push_to_talk_modifiers();

        if !is_release && keyval == IBUS_KEY_Escape && self.is_recording {
            debug!("Escape pressed in push-to-talk mode, cancelling recording");
            self.ptt_pressed = false;
            self.cancel_recording();
            return TRUE;
        }

        if normalize_keyval(keyval) != normalize_keyval(ptt_keyval) {
            return 0;
        }

        if is_release {
            if self.ptt_pressed {
                self.ptt_pressed = false;
                let restore_engine = self.last_non_handy_engine.clone();
                self.stop_and_commit(engine, restore_engine, false);
                return TRUE;
            }
            return 0;
        }

        if !self.matches_required_modifiers(modifiers, ptt_modifiers) {
            return 0;
        }

        self.ptt_pressed = true;
        if !self.is_recording {
            self.start_recording(engine);
        }
        TRUE
    }

    fn matches_required_modifiers(&self, modifiers: guint, required_modifiers: guint) -> bool {
        let effective_modifiers = modifiers & !IBUS_RELEASE_MASK;
        if required_modifiers == 0 {
            effective_modifiers == 0
        } else {
            (effective_modifiers & required_modifiers) == required_modifiers
        }
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
                if let Some(target_engine) = restore_engine.clone() {
                    switch_engine_async(target_engine);
                }
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
                if let Some(target_engine) = restore_engine {
                    switch_engine_async(target_engine);
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

    fn refresh_last_non_handy_engine(&mut self) {
        if let Some(engine_name) = current_ibus_engine() {
            if !is_handy_engine(&engine_name) {
                self.last_non_handy_engine = Some(engine_name);
            }
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

fn normalize_keyval(keyval: u32) -> u32 {
    if (b'A' as u32..=b'Z' as u32).contains(&keyval) {
        keyval + (b'a' - b'A') as u32
    } else {
        keyval
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

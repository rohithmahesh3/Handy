use std::ffi::{CStr, CString};

use ibus_sys::{gboolean, gchar, ibus_handy_get_global_engine_name, ibus_handy_set_global_engine};
use log::{info, warn};

pub const HANDY_ENGINE_NAME: &str = "handy";

pub fn current_ibus_engine() -> Option<String> {
    let engine_ptr = unsafe { ibus_handy_get_global_engine_name() };
    if engine_ptr.is_null() {
        return None;
    }

    let engine = unsafe { CStr::from_ptr(engine_ptr as *const i8) }
        .to_string_lossy()
        .trim()
        .to_string();
    unsafe {
        glib::ffi::g_free(engine_ptr as *mut _);
    }

    if engine.is_empty() {
        None
    } else {
        Some(engine)
    }
}

pub fn is_handy_engine(engine_name: &str) -> bool {
    engine_name == HANDY_ENGINE_NAME || engine_name.ends_with(":handy")
}

pub fn switch_engine(engine_name: &str) -> bool {
    if engine_name.is_empty() {
        return false;
    }

    let c_engine = match CString::new(engine_name) {
        Ok(value) => value,
        Err(e) => {
            warn!("Invalid engine name '{}': {}", engine_name, e);
            return false;
        }
    };

    let result: gboolean =
        unsafe { ibus_handy_set_global_engine(c_engine.as_ptr() as *const gchar) };
    result == ibus_sys::TRUE
}

pub fn switch_engine_async(engine_name: String) {
    if engine_name.is_empty() {
        return;
    }

    std::thread::spawn(move || {
        if switch_engine(&engine_name) {
            info!("Switched input source to {}", engine_name);
        } else {
            warn!("Failed to switch input source to {}", engine_name);
        }
    });
}

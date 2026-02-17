use std::ffi::{CStr, CString};

use anyhow::{anyhow, Result};
use ibus_sys::{
    gboolean, gchar, ibus_handy_daemon_get_global_engine_name, ibus_handy_daemon_set_global_engine,
};

pub const HANDY_ENGINE_NAME: &str = "handy";
const HANDY_ENGINE_FALLBACK_NAME: &str = "other:handy";

pub fn get_current_engine() -> Result<String> {
    let engine_ptr = unsafe { ibus_handy_daemon_get_global_engine_name() };
    if engine_ptr.is_null() {
        return Err(anyhow!("IBus returned empty global engine"));
    }

    let engine = unsafe { CStr::from_ptr(engine_ptr as *const i8) }
        .to_string_lossy()
        .trim()
        .to_string();
    unsafe {
        glib::ffi::g_free(engine_ptr as *mut _);
    }

    if engine.is_empty() {
        Err(anyhow!("IBus returned blank global engine"))
    } else {
        Ok(engine)
    }
}

pub fn set_global_engine(engine_name: &str) -> Result<()> {
    if engine_name.trim().is_empty() {
        return Err(anyhow!("Target engine name is empty"));
    }

    let c_engine = CString::new(engine_name)
        .map_err(|e| anyhow!("Invalid engine name '{}': {}", engine_name, e))?;

    let result: gboolean =
        unsafe { ibus_handy_daemon_set_global_engine(c_engine.as_ptr() as *const gchar) };
    if result == ibus_sys::TRUE {
        Ok(())
    } else {
        Err(anyhow!(
            "IBus rejected global engine switch to '{}'",
            engine_name
        ))
    }
}

pub fn is_handy_engine(engine_name: &str) -> bool {
    engine_name == HANDY_ENGINE_NAME || engine_name.ends_with(":handy")
}

pub fn switch_to_handy_engine() -> Result<String> {
    if let Ok(engine) = get_current_engine() {
        if is_handy_engine(&engine) {
            return Ok(engine);
        }
    }

    let mut attempts = Vec::new();
    for candidate in [HANDY_ENGINE_NAME, HANDY_ENGINE_FALLBACK_NAME] {
        match set_global_engine(candidate) {
            Ok(()) => return Ok(candidate.to_string()),
            Err(e) => attempts.push(format!("{} ({})", candidate, e)),
        }
    }

    Err(anyhow!(
        "Failed to switch to Handy input source. Tried: {}",
        attempts.join(", ")
    ))
}

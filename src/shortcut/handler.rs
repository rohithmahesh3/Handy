pub fn init_shortcuts() {
    log::info!("Shortcuts not needed in IBus mode");
}

pub fn change_binding(_id: &str, _binding: &str) -> Result<(), String> {
    Ok(())
}

pub fn reset_binding(_id: &str) -> Result<(), String> {
    Ok(())
}

pub fn suspend_binding(_id: &str) {
    log::debug!("Suspend binding: {}", _id);
}

pub fn resume_binding(_id: &str) {
    log::debug!("Resume binding: {}", _id);
}

pub fn unregister_cancel_shortcut() {
    log::debug!("Unregister cancel shortcut");
}

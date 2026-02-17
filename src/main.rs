fn main() {
    let daemon_mode = std::env::args().any(|arg| arg == "--daemon");
    if daemon_mode {
        handy_app_lib::app::run_daemon();
    } else {
        handy_app_lib::app::run_ui();
    }
}

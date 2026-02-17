use super::Page;
use crate::app::AppState;
use crate::utils::logging::read_recent_logs;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Label, Orientation, ScrolledWindow, TextView, Widget};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zbus::blocking::Connection;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";
const MAX_LOG_LINES: usize = 400;
const UI_POLL_INTERVAL_MS: u64 = 80;

pub struct DebugPage {
    container: Box,
    is_recording: Arc<AtomicBool>,
}

impl DebugPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let container = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let test_group = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .build();
        let test_title = Label::builder()
            .label("Transcription Testing")
            .css_classes(["title-4"])
            .halign(Align::Start)
            .build();
        test_group.append(&test_title);
        let test_help = Label::builder()
            .label("Click Start, speak, then click Stop to transcribe into the box below.")
            .halign(Align::Start)
            .wrap(true)
            .xalign(0.0)
            .build();
        test_group.append(&test_help);

        let controls_box = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        let start_btn = Button::with_label("Start Recording");
        let stop_btn = Button::with_label("Stop & Transcribe");
        stop_btn.set_sensitive(false);
        let clear_btn = Button::with_label("Clear");
        controls_box.append(&start_btn);
        controls_box.append(&stop_btn);
        controls_box.append(&clear_btn);
        test_group.append(&controls_box);

        let status_label = Label::builder()
            .label("Idle")
            .halign(Align::Start)
            .xalign(0.0)
            .build();
        test_group.append(&status_label);

        let output_buffer = gtk4::TextBuffer::new(None);
        output_buffer.set_text("No transcription yet.");
        let output_view = TextView::builder()
            .buffer(&output_buffer)
            .editable(false)
            .vexpand(false)
            .hexpand(true)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .build();
        let output_scaffold = ScrolledWindow::builder()
            .min_content_height(140)
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&output_view)
            .build();
        test_group.append(&output_scaffold);
        container.append(&test_group);

        let section_separator = gtk4::Separator::builder()
            .orientation(Orientation::Horizontal)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        container.append(&section_separator);

        let is_recording = Arc::new(AtomicBool::new(false));
        let request_in_flight = Arc::new(AtomicBool::new(false));
        let update_controls = Rc::new({
            let start_btn = start_btn.clone();
            let stop_btn = stop_btn.clone();
            let is_recording = is_recording.clone();
            let request_in_flight = request_in_flight.clone();
            move || {
                let recording = is_recording.load(Ordering::SeqCst);
                let in_flight = request_in_flight.load(Ordering::SeqCst);
                start_btn.set_sensitive(!recording && !in_flight);
                stop_btn.set_sensitive(recording && !in_flight);
            }
        });

        start_btn.connect_clicked({
            let status_label = status_label.clone();
            let is_recording = is_recording.clone();
            let request_in_flight = request_in_flight.clone();
            let update_controls = update_controls.clone();
            move |_| {
                if request_in_flight
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return;
                }
                status_label.set_text("Starting recording...");
                update_controls();

                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(call_start_recording());
                });

                let status_label = status_label.clone();
                let is_recording = is_recording.clone();
                let request_in_flight = request_in_flight.clone();
                let update_controls = update_controls.clone();
                glib::timeout_add_local(
                    std::time::Duration::from_millis(UI_POLL_INTERVAL_MS),
                    move || match rx.try_recv() {
                        Ok(result) => {
                            request_in_flight.store(false, Ordering::SeqCst);
                            match result {
                                Ok(()) => {
                                    is_recording.store(true, Ordering::SeqCst);
                                    status_label.set_text("Recording...");
                                }
                                Err(e) => {
                                    is_recording.store(false, Ordering::SeqCst);
                                    status_label.set_text(&format!("Error: {}", e));
                                }
                            }
                            update_controls();
                            glib::ControlFlow::Break
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            request_in_flight.store(false, Ordering::SeqCst);
                            is_recording.store(false, Ordering::SeqCst);
                            status_label.set_text("Error: start worker disconnected");
                            update_controls();
                            glib::ControlFlow::Break
                        }
                    },
                );
            }
        });

        stop_btn.connect_clicked({
            let output_buffer = output_buffer.clone();
            let status_label = status_label.clone();
            let is_recording = is_recording.clone();
            let request_in_flight = request_in_flight.clone();
            let update_controls = update_controls.clone();
            move |_| {
                if request_in_flight
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return;
                }
                status_label.set_text("Stopping and transcribing...");
                update_controls();

                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(call_stop_recording());
                });

                let output_buffer = output_buffer.clone();
                let status_label = status_label.clone();
                let is_recording = is_recording.clone();
                let request_in_flight = request_in_flight.clone();
                let update_controls = update_controls.clone();
                glib::timeout_add_local(
                    std::time::Duration::from_millis(UI_POLL_INTERVAL_MS),
                    move || match rx.try_recv() {
                        Ok(result) => {
                            request_in_flight.store(false, Ordering::SeqCst);
                            is_recording.store(false, Ordering::SeqCst);
                            match result {
                                Ok(text) => {
                                    let final_text = if text.trim().is_empty() {
                                        "No speech detected.".to_string()
                                    } else {
                                        text
                                    };
                                    output_buffer.set_text(&final_text);
                                    status_label.set_text("Idle");
                                }
                                Err(e) => {
                                    status_label.set_text(&format!("Error: {}", e));
                                }
                            }
                            update_controls();
                            glib::ControlFlow::Break
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            request_in_flight.store(false, Ordering::SeqCst);
                            is_recording.store(false, Ordering::SeqCst);
                            status_label.set_text("Error: stop worker disconnected");
                            update_controls();
                            glib::ControlFlow::Break
                        }
                    },
                );
            }
        });

        clear_btn.connect_clicked({
            let output_buffer = output_buffer.clone();
            move |_| {
                output_buffer.set_text("No transcription yet.");
            }
        });

        {
            let status_label = status_label.clone();
            let is_recording = is_recording.clone();
            let update_controls = update_controls.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(call_recording_state());
            });
            glib::timeout_add_local(
                std::time::Duration::from_millis(UI_POLL_INTERVAL_MS),
                move || match rx.try_recv() {
                    Ok(result) => {
                        match result {
                            Ok(recording) => {
                                is_recording.store(recording, Ordering::SeqCst);
                                if recording {
                                    status_label.set_text("Recording...");
                                }
                            }
                            Err(e) => {
                                status_label.set_text(&format!("Status unavailable: {}", e));
                            }
                        }
                        update_controls();
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                },
            );
        }

        let header_box = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(12)
            .build();

        let title = gtk4::Label::builder()
            .label("Debug Logs")
            .css_classes(["title-2"])
            .halign(Align::Start)
            .hexpand(true)
            .build();
        header_box.append(&title);

        let refresh_btn = Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh Logs")
            .build();

        let log_buffer = state.log_buffer.clone();
        let text_buffer = gtk4::TextBuffer::new(None);

        refresh_debug_view(&text_buffer, &log_buffer);

        refresh_btn.connect_clicked({
            let log_buffer = log_buffer.clone();
            let text_buffer = text_buffer.clone();
            move |_| {
                refresh_debug_view(&text_buffer, &log_buffer);
            }
        });

        header_box.append(&refresh_btn);
        container.append(&header_box);

        let scaffold = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .build();

        let text_view = TextView::builder()
            .buffer(&text_buffer)
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .build();

        scaffold.set_child(Some(&text_view));
        container.append(&scaffold);

        let log_buffer_clone = log_buffer.clone();
        let text_buffer_clone = text_buffer.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            refresh_debug_view(&text_buffer_clone, &log_buffer_clone);
            glib::ControlFlow::Continue
        });

        Self {
            container,
            is_recording,
        }
    }
}

impl Page for DebugPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

impl Drop for DebugPage {
    fn drop(&mut self) {
        if self.is_recording.load(Ordering::SeqCst) {
            std::thread::spawn(|| {
                let _ = call_cancel_recording();
            });
        }
    }
}

fn refresh_debug_view(
    text_buffer: &gtk4::TextBuffer,
    ui_log_buffer: &Arc<Mutex<VecDeque<String>>>,
) {
    let ui_logs = read_recent_logs(ui_log_buffer, MAX_LOG_LINES);
    let daemon_logs = fetch_daemon_logs(MAX_LOG_LINES);
    let ptt_diagnostics = fetch_ptt_diagnostics_summary();
    let rendered = render_debug_text(&ui_logs, daemon_logs.as_ref(), ptt_diagnostics.as_ref());
    text_buffer.set_text(&rendered);
}

fn fetch_daemon_logs(limit: usize) -> Result<Vec<String>, String> {
    let conn =
        Connection::session().map_err(|e| format!("Cannot connect to session bus: {}", e))?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "GetRecentLogs",
            &(),
        )
        .map_err(|e| format!("Daemon log query failed: {}", e))?;

    let logs = reply
        .body()
        .deserialize::<Vec<String>>()
        .map_err(|e| format!("Invalid daemon log payload: {}", e))?;

    let start = logs.len().saturating_sub(limit);
    Ok(logs.into_iter().skip(start).collect())
}

fn fetch_ptt_diagnostics_summary() -> Result<String, String> {
    let conn =
        Connection::session().map_err(|e| format!("Cannot connect to session bus: {}", e))?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "GetPttDiagnosticsVerbose",
            &(),
        )
        .map_err(|e| format!("PTT diagnostics query failed: {}", e))?;

    let payload = reply
        .body()
        .deserialize::<String>()
        .map_err(|e| format!("Invalid PTT diagnostics payload: {}", e))?;
    let diagnostics: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|e| format!("Invalid PTT diagnostics JSON: {}", e))?;

    let healthy = diagnostics
        .get("healthy")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let code = diagnostics
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let message = diagnostics
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let listener_session_ok = diagnostics
        .get("listener_session_ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let shortcut_bound = diagnostics
        .get("shortcut_bound")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let bind_fail_count = diagnostics
        .get("bind_fail_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let press_while_handy_count = diagnostics
        .get("press_while_handy_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let release_timeout_fallback_count = diagnostics
        .get("release_timeout_fallback_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let current_state = diagnostics
        .get("current_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let shortcut_description = diagnostics
        .get("shortcut_description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let last_start_failure_code = diagnostics
        .get("last_start_failure_code")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let last_start_failure_message = diagnostics
        .get("last_start_failure_message")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let last_dbus_error = diagnostics
        .get("last_dbus_error")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(format!(
        "healthy={} code={} message={} state={} shortcut='{}' listener_ok={} shortcut_bound={} bind_failures={} press_while_handy={} stop_fallbacks={} start_failure_code={} start_failure_message={} last_dbus_error={}",
        healthy,
        code,
        message,
        current_state,
        shortcut_description,
        listener_session_ok,
        shortcut_bound,
        bind_fail_count,
        press_while_handy_count,
        release_timeout_fallback_count,
        last_start_failure_code,
        last_start_failure_message,
        last_dbus_error
    ))
}

fn render_debug_text(
    ui_logs: &[String],
    daemon_logs: Result<&Vec<String>, &String>,
    ptt_diagnostics: Result<&String, &String>,
) -> String {
    let mut out = String::new();

    out.push_str("=== Push-to-Talk Diagnostics ===\n");
    match ptt_diagnostics {
        Ok(summary) => {
            out.push_str("[ptt] ");
            out.push_str(summary);
            out.push('\n');
        }
        Err(err) => {
            out.push_str("[ptt] unavailable: ");
            out.push_str(err);
            out.push('\n');
        }
    }

    out.push('\n');
    out.push_str("=== UI Process Logs ===\n");
    if ui_logs.is_empty() {
        out.push_str("[ui] <no logs yet>\n");
    } else {
        for line in ui_logs {
            out.push_str("[ui] ");
            out.push_str(line);
            out.push('\n');
        }
    }

    out.push('\n');
    out.push_str("=== Daemon Process Logs ===\n");
    match daemon_logs {
        Ok(logs) if logs.is_empty() => out.push_str("[daemon] <no logs yet>\n"),
        Ok(logs) => {
            for line in logs {
                out.push_str("[daemon] ");
                out.push_str(line);
                out.push('\n');
            }
        }
        Err(err) => {
            out.push_str("[daemon] unavailable: ");
            out.push_str(err);
            out.push('\n');
        }
    }

    out
}

fn call_start_recording() -> Result<(), String> {
    let conn = Connection::session().map_err(|e| format!("Session bus unavailable: {}", e))?;
    conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        "StartRecording",
        &(),
    )
    .map_err(|e| format!("StartRecording failed: {}", e))?;
    Ok(())
}

fn call_stop_recording() -> Result<String, String> {
    let conn = Connection::session().map_err(|e| format!("Session bus unavailable: {}", e))?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "StopRecording",
            &(),
        )
        .map_err(|e| format!("StopRecording failed: {}", e))?;

    reply
        .body()
        .deserialize::<String>()
        .map_err(|e| format!("Failed to decode StopRecording response: {}", e))
}

fn call_cancel_recording() -> Result<(), String> {
    let conn = Connection::session().map_err(|e| format!("Session bus unavailable: {}", e))?;
    conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        "CancelRecording",
        &(),
    )
    .map_err(|e| format!("CancelRecording failed: {}", e))?;
    Ok(())
}

fn call_recording_state() -> Result<bool, String> {
    let conn = Connection::session().map_err(|e| format!("Session bus unavailable: {}", e))?;
    let reply = conn
        .call_method(
            Some(HANDY_BUS_NAME),
            HANDY_OBJECT_PATH,
            Some(HANDY_INTERFACE),
            "GetState",
            &(),
        )
        .map_err(|e| format!("GetState failed: {}", e))?;

    let (is_recording, _has_model): (bool, bool) = reply
        .body()
        .deserialize()
        .map_err(|e| format!("Failed to decode GetState response: {}", e))?;
    Ok(is_recording)
}

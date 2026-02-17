use gtk4::prelude::*;
use gtk4::{Align, Box, ComboBoxText, Orientation, PolicyType, ScrolledWindow, Switch, Widget};
use libadwaita::prelude::{ActionRowExt, PreferencesGroupExt};
use libadwaita::{ActionRow, Clamp, PreferencesGroup};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zbus::blocking::Connection;

use super::Page;
use crate::app::AppState;
use crate::global_shortcuts::{
    authorize_shortcut_interactively_from_ui, request_shortcut_listener_rebind,
};
use crate::settings::ModelUnloadTimeout;

pub struct AdvancedPage {
    container: ScrolledWindow,
}

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

impl AdvancedPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let main_box = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(12)
            .hexpand(true)
            .vexpand(true)
            .build();
        main_box.set_margin_top(24);
        main_box.set_margin_bottom(24);
        main_box.set_margin_start(24);
        main_box.set_margin_end(24);

        let model_group = PreferencesGroup::builder().title("Model").build();

        let timeout_row = ActionRow::builder()
            .title("Unload Model After")
            .subtitle("Free memory when idle")
            .build();

        let timeout_combo = ComboBoxText::new();
        let timeouts = [
            (ModelUnloadTimeout::Never, "Never"),
            (ModelUnloadTimeout::Immediately, "Immediately"),
            (ModelUnloadTimeout::Sec5, "5 seconds"),
            (ModelUnloadTimeout::Min2, "2 minutes"),
            (ModelUnloadTimeout::Min5, "5 minutes"),
            (ModelUnloadTimeout::Min10, "10 minutes"),
            (ModelUnloadTimeout::Min15, "15 minutes"),
            (ModelUnloadTimeout::Hour1, "1 hour"),
        ];

        let current_timeout = state.settings.model_unload_timeout();
        let mut timeout_index = 0;
        for (i, (timeout, name)) in timeouts.iter().enumerate() {
            timeout_combo.append(Some(&format!("{}", i)), name);
            if *timeout == current_timeout {
                timeout_index = i as u32;
            }
        }
        timeout_combo.set_active(Some(timeout_index));

        let state_clone = state.clone();
        timeout_combo.connect_changed(move |combo| {
            if let Some(id) = combo.active_id() {
                if let Ok(idx) = id.parse::<usize>() {
                    if idx < timeouts.len() {
                        state_clone
                            .settings
                            .set_model_unload_timeout(timeouts[idx].0);
                    }
                }
            }
        });
        timeout_row.add_suffix(&timeout_combo);
        model_group.add(&timeout_row);

        main_box.append(&model_group);

        let debug_group = PreferencesGroup::builder().title("Debug").build();

        let debug_row = ActionRow::builder()
            .title("Debug Mode")
            .subtitle("Enable verbose logging")
            .build();

        let debug_switch = Switch::builder()
            .active(state.settings.debug_mode())
            .build();
        debug_switch.set_valign(Align::Center);
        debug_switch.set_vexpand(false);
        debug_switch.set_hexpand(false);
        debug_switch.set_halign(Align::End);

        let state_clone = state.clone();
        debug_switch.connect_active_notify(move |switch| {
            state_clone.settings.set_debug_mode(switch.is_active());
        });
        debug_row.add_suffix(&debug_switch);
        debug_group.add(&debug_row);

        let experimental_row = ActionRow::builder()
            .title("Experimental Features")
            .subtitle("Enable beta features")
            .build();

        let experimental_switch = Switch::builder()
            .active(state.settings.experimental_enabled())
            .build();
        experimental_switch.set_valign(Align::Center);
        experimental_switch.set_vexpand(false);
        experimental_switch.set_hexpand(false);
        experimental_switch.set_halign(Align::End);

        let state_clone = state.clone();
        experimental_switch.connect_active_notify(move |switch| {
            state_clone
                .settings
                .set_experimental_enabled(switch.is_active());
        });
        experimental_row.add_suffix(&experimental_switch);
        debug_group.add(&experimental_row);

        main_box.append(&debug_group);

        let diagnostics_group = PreferencesGroup::builder()
            .title("Push-to-Talk Diagnostics")
            .build();

        let status_row = ActionRow::builder()
            .title("Global Shortcut Status")
            .subtitle("Checking daemon health...")
            .build();
        let refresh_button = gtk4::Button::with_label("Refresh");
        refresh_button.add_css_class("flat");
        let status_row_for_click = status_row.clone();
        refresh_button.connect_clicked(move |_| {
            status_row_for_click.set_subtitle(&load_ptt_diagnostics_subtitle());
        });
        status_row.add_suffix(&refresh_button);
        diagnostics_group.add(&status_row);

        let help_row = ActionRow::builder()
            .title("Recovery Hint")
            .subtitle("If unhealthy, keep Handy daemon running and re-save push-to-talk shortcut.")
            .build();
        diagnostics_group.add(&help_row);

        let authorize_row = ActionRow::builder()
            .title("Authorize Global Shortcut")
            .subtitle("Run interactive portal authorization from this window")
            .build();
        let authorize_button = gtk4::Button::with_label("Authorize Now");
        authorize_button.add_css_class("suggested-action");
        let status_row_for_auth = status_row.clone();
        authorize_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            button.set_label("Authorizing...");
            let button_weak = button.downgrade();
            let status_row = status_row_for_auth.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(authorize_shortcut_interactively_from_ui());
            });
            glib::timeout_add_local(Duration::from_millis(120), move || match rx.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(trigger) => {
                            status_row.set_subtitle(&format!(
                                "Authorization succeeded for '{}'",
                                trigger
                            ));
                            request_shortcut_listener_rebind();
                        }
                        Err(e) => {
                            status_row.set_subtitle(&format!("Authorization failed: {}", e));
                        }
                    }
                    if let Some(button) = button_weak.upgrade() {
                        button.set_sensitive(true);
                        button.set_label("Authorize Now");
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let Some(button) = button_weak.upgrade() {
                        button.set_sensitive(true);
                        button.set_label("Authorize Now");
                    }
                    status_row.set_subtitle("Authorization failed: worker disconnected");
                    glib::ControlFlow::Break
                }
            });
        });
        authorize_row.add_suffix(&authorize_button);
        diagnostics_group.add(&authorize_row);
        main_box.append(&diagnostics_group);

        status_row.set_subtitle(&load_ptt_diagnostics_subtitle());
        let status_row_for_timer = status_row.clone();
        glib::timeout_add_local(Duration::from_secs(4), move || {
            status_row_for_timer.set_subtitle(&load_ptt_diagnostics_subtitle());
            glib::ControlFlow::Continue
        });

        let clamp = Clamp::builder()
            .maximum_size(900)
            .tightening_threshold(600)
            .child(&main_box)
            .build();

        let container = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .child(&clamp)
            .build();

        Self { container }
    }
}

impl Page for AdvancedPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

fn load_ptt_diagnostics_subtitle() -> String {
    let conn = match Connection::session() {
        Ok(conn) => conn,
        Err(e) => return format!("Unavailable: cannot connect to session bus ({})", e),
    };

    let reply = match conn.call_method(
        Some(HANDY_BUS_NAME),
        HANDY_OBJECT_PATH,
        Some(HANDY_INTERFACE),
        "GetPttDiagnostics",
        &(),
    ) {
        Ok(reply) => reply,
        Err(e) => return format!("Unavailable: daemon not responding ({})", e),
    };

    let diagnostics: (bool, String, String, String, u64, bool, bool, u64, u64, u64) =
        match reply.body().deserialize() {
            Ok(tuple) => tuple,
            Err(e) => return format!("Unavailable: invalid diagnostics payload ({})", e),
        };

    let (
        healthy,
        _component,
        code,
        message,
        last_success_ms,
        portal_session_ok,
        shortcut_bound,
        portal_bind_fail_count,
        press_while_handy_count,
        release_timeout_fallback_count,
    ) = diagnostics;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let age_seconds = if last_success_ms == 0 {
        None
    } else {
        Some(now_ms.saturating_sub(last_success_ms) / 1000)
    };

    if healthy {
        match age_seconds {
            Some(age) => format!(
                "Healthy | session={} shortcut={} | last ok {}s ago",
                portal_session_ok, shortcut_bound, age
            ),
            None => format!(
                "Healthy | session={} shortcut={} | last ok unknown",
                portal_session_ok, shortcut_bound
            ),
        }
    } else {
        format!(
            "Unhealthy ({}) | {} | bind_failures={} press_while_handy={} watchdog_fallbacks={}",
            code,
            message,
            portal_bind_fail_count,
            press_while_handy_count,
            release_timeout_fallback_count
        )
    }
}

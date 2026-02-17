use gtk4::prelude::*;
use gtk4::{
    Adjustment, Align, Box, ComboBoxText, Orientation, PolicyType, Scale, ScrolledWindow, Switch,
    Widget,
};
use libadwaita::prelude::{ActionRowExt, PreferencesGroupExt};
use libadwaita::{ActionRow, Clamp, PreferencesGroup};
use std::sync::Arc;

use super::Page;
use crate::app::AppState;
use crate::settings::RecordingMode;

const PTT_PRESETS: [(&str, &str, u32, u32); 3] = [
    ("ctrl_space", "Ctrl+Space", 32, 4),
    ("alt_space", "Alt+Space", 32, 8),
    ("ctrl_shift_space", "Ctrl+Shift+Space", 32, 5),
];

pub struct GeneralPage {
    container: ScrolledWindow,
}

impl GeneralPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let container = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .build();

        let vbox = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(24)
            .hexpand(true)
            .vexpand(true)
            .build();
        vbox.set_margin_top(24);
        vbox.set_margin_bottom(24);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);

        let recording_group = PreferencesGroup::builder()
            .title("Recording")
            .description(
                "Auto starts on Handy source switch. Push-to-talk records while key is held.",
            )
            .build();

        let mode_row = ActionRow::builder()
            .title("Recording Mode")
            .subtitle("Choose automatic or push-to-talk triggering")
            .build();
        let mode_combo = ComboBoxText::new();
        mode_combo.append(Some("auto"), "Auto");
        mode_combo.append(Some("push_to_talk"), "Push-to-talk");
        match state.settings.recording_mode() {
            RecordingMode::Auto => {
                mode_combo.set_active_id(Some("auto"));
            }
            RecordingMode::PushToTalk => {
                mode_combo.set_active_id(Some("push_to_talk"));
            }
        }
        mode_row.add_suffix(&mode_combo);
        recording_group.add(&mode_row);

        let ptt_row = ActionRow::builder()
            .title("Push-to-Talk Shortcut")
            .subtitle("Shortcut used to start and stop recording in push-to-talk mode")
            .build();
        let ptt_combo = ComboBoxText::new();
        for (id, label, _, _) in PTT_PRESETS {
            ptt_combo.append(Some(id), label);
        }
        let current_keyval = state.settings.push_to_talk_keyval();
        let current_modifiers = state.settings.push_to_talk_modifiers();
        if let Some(id) = ptt_preset_id(current_keyval, current_modifiers) {
            ptt_combo.set_active_id(Some(id));
        } else {
            let custom_label = format!(
                "Custom ({})",
                format_shortcut_label(current_keyval, current_modifiers)
            );
            ptt_combo.append(Some("custom"), &custom_label);
            ptt_combo.set_active_id(Some("custom"));
        }
        ptt_row.set_sensitive(state.settings.recording_mode() == RecordingMode::PushToTalk);
        ptt_row.add_suffix(&ptt_combo);
        recording_group.add(&ptt_row);

        let mute_row = ActionRow::builder()
            .title("Mute While Recording")
            .subtitle("Mute system audio during recording")
            .build();
        let mute_switch = Switch::builder()
            .active(state.settings.mute_while_recording())
            .build();
        mute_switch.set_valign(Align::Center);
        mute_switch.set_vexpand(false);
        mute_switch.set_hexpand(false);
        mute_switch.set_halign(Align::End);
        mute_row.add_suffix(&mute_switch);
        mute_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_mute_while_recording(switch.is_active());
            }
        });
        recording_group.add(&mute_row);

        mode_combo.connect_changed({
            let settings = state.settings.clone();
            let ptt_row = ptt_row.clone();
            move |combo| {
                let mode = match combo.active_id().as_deref() {
                    Some("push_to_talk") => RecordingMode::PushToTalk,
                    _ => RecordingMode::Auto,
                };
                settings.set_recording_mode(mode);
                ptt_row.set_sensitive(mode == RecordingMode::PushToTalk);
            }
        });

        ptt_combo.connect_changed({
            let settings = state.settings.clone();
            move |combo| {
                let Some(active_id) = combo.active_id() else {
                    return;
                };
                if let Some((_, _, keyval, modifiers)) =
                    PTT_PRESETS.iter().find(|(id, _, _, _)| *id == active_id)
                {
                    settings.set_push_to_talk_keyval(*keyval);
                    settings.set_push_to_talk_modifiers(*modifiers);
                }
            }
        });

        vbox.append(&recording_group);

        let audio_feedback_group = PreferencesGroup::builder().title("Audio Feedback").build();

        let feedback_row = ActionRow::builder()
            .title("Play Sounds")
            .subtitle("Play sound on start/stop")
            .build();
        let feedback_switch = Switch::builder()
            .active(state.settings.audio_feedback())
            .build();
        feedback_switch.set_valign(Align::Center);
        feedback_switch.set_vexpand(false);
        feedback_switch.set_hexpand(false);
        feedback_switch.set_halign(Align::End);
        feedback_row.add_suffix(&feedback_switch);
        feedback_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_audio_feedback(switch.is_active());
            }
        });
        audio_feedback_group.add(&feedback_row);

        let volume_row = ActionRow::builder().title("Volume").build();
        let volume_scale = Scale::builder()
            .adjustment(&Adjustment::new(
                state.settings.audio_feedback_volume() as f64,
                0.0,
                1.0,
                0.1,
                0.1,
                0.1,
            ))
            .hexpand(true)
            .build();
        volume_scale.connect_value_changed({
            let settings = state.settings.clone();
            move |scale| {
                settings.set_audio_feedback_volume(scale.value() as f32);
            }
        });
        volume_row.add_suffix(&volume_scale);
        audio_feedback_group.add(&volume_row);

        vbox.append(&audio_feedback_group);

        let language_group = PreferencesGroup::builder().title("Language").build();

        let lang_row = ActionRow::builder()
            .title("Transcription Language")
            .subtitle("Language for transcription")
            .build();

        let language_combo = ComboBoxText::new();
        let languages = [
            ("auto", "Auto Detect"),
            ("en", "English"),
            ("zh", "Chinese"),
            ("zh-Hans", "Chinese (Simplified)"),
            ("zh-Hant", "Chinese (Traditional)"),
            ("de", "German"),
            ("es", "Spanish"),
            ("fr", "French"),
            ("ja", "Japanese"),
            ("ko", "Korean"),
            ("pt", "Portuguese"),
            ("ru", "Russian"),
            ("it", "Italian"),
        ];

        let selected_lang = state.settings.selected_language();
        let mut selected_index = 0;
        for (i, (code, name)) in languages.iter().enumerate() {
            language_combo.append(Some(code), name);
            if *code == selected_lang {
                selected_index = i as u32;
            }
        }
        language_combo.set_active(Some(selected_index));

        let state_clone = state.clone();
        language_combo.connect_changed(move |combo| {
            if let Some(active) = combo.active_id() {
                state_clone.settings.set_selected_language(&active);
            }
        });
        lang_row.add_suffix(&language_combo);
        language_group.add(&lang_row);

        let translate_row = ActionRow::builder()
            .title("Translate to English")
            .subtitle("Translate non-English speech to English")
            .build();
        let translate_switch = Switch::builder()
            .active(state.settings.translate_to_english())
            .build();
        translate_switch.set_valign(Align::Center);
        translate_switch.set_vexpand(false);
        translate_switch.set_hexpand(false);
        translate_switch.set_halign(Align::End);
        translate_row.add_suffix(&translate_switch);
        translate_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_translate_to_english(switch.is_active());
            }
        });
        language_group.add(&translate_row);

        vbox.append(&language_group);

        let clamp = Clamp::builder()
            .maximum_size(900)
            .tightening_threshold(600)
            .build();
        clamp.set_child(Some(&vbox));

        container.set_child(Some(&clamp));

        Self { container }
    }
}

impl Page for GeneralPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

fn ptt_preset_id(keyval: u32, modifiers: u32) -> Option<&'static str> {
    PTT_PRESETS
        .iter()
        .find(|(_, _, preset_keyval, preset_modifiers)| {
            *preset_keyval == keyval && *preset_modifiers == modifiers
        })
        .map(|(id, _, _, _)| *id)
}

fn format_shortcut_label(keyval: u32, modifiers: u32) -> String {
    format!("key {} + mods {}", keyval, modifiers)
}

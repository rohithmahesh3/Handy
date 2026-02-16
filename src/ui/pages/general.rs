use gtk4::prelude::*;
use gtk4::{Adjustment, ComboBoxText, PolicyType, Scale, ScrolledWindow, Switch, Widget};
use libadwaita::prelude::{ActionRowExt, PreferencesGroupExt, PreferencesPageExt};
use libadwaita::{ActionRow, Clamp, PreferencesGroup, PreferencesPage};
use std::sync::Arc;

use super::Page;
use crate::app::AppState;

pub struct GeneralPage {
    container: ScrolledWindow,
}

impl GeneralPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let container = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .build();

        let clamp = Clamp::builder()
            .maximum_size(900)
            .tightening_threshold(700)
            .build();
        clamp.set_margin_top(24);
        clamp.set_margin_bottom(24);
        clamp.set_margin_start(24);
        clamp.set_margin_end(24);

        let page = PreferencesPage::new();

        let recording_group = PreferencesGroup::builder()
            .title("Recording")
            .description("Use Super+Space to switch to Handy IM and start recording")
            .build();

        let mute_row = ActionRow::builder()
            .title("Mute While Recording")
            .subtitle("Mute system audio during recording")
            .build();
        let mute_switch = Switch::builder()
            .active(state.settings.mute_while_recording())
            .build();
        mute_row.add_suffix(&mute_switch);
        mute_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_mute_while_recording(switch.is_active());
            }
        });
        recording_group.add(&mute_row);

        page.add(&recording_group);

        let audio_feedback_group = PreferencesGroup::builder().title("Audio Feedback").build();

        let feedback_row = ActionRow::builder()
            .title("Play Sounds")
            .subtitle("Play sound on start/stop")
            .build();
        let feedback_switch = Switch::builder()
            .active(state.settings.audio_feedback())
            .build();
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
        volume_row.add_suffix(&volume_scale);
        audio_feedback_group.add(&volume_row);

        page.add(&audio_feedback_group);

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
        translate_row.add_suffix(&translate_switch);
        translate_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_translate_to_english(switch.is_active());
            }
        });
        language_group.add(&translate_row);

        page.add(&language_group);

        clamp.set_child(Some(&page));
        container.set_child(Some(&clamp));

        Self { container }
    }
}

impl Page for GeneralPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

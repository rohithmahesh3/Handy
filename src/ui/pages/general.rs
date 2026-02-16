use gtk::prelude::*;
use gtk::{Widget, Box, Orientation, ScrolledWindow};
use libadwaita::{PreferencesGroup, ActionRow, Switch, ComboRow};
use std::sync::Arc;

use crate::app::AppState;
use super::Page;

pub struct GeneralPage {
    container: ScrolledWindow,
}

impl GeneralPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let container = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let vbox = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(24)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let shortcut_group = PreferencesGroup::builder()
            .title("Keyboard Shortcut")
            .build();

        let shortcut_row = ActionRow::builder()
            .title("Transcription Shortcut")
            .subtitle("Press to record shortcut")
            .build();
        shortcut_group.add(&shortcut_row);

        vbox.append(&shortcut_group);

        let recording_group = PreferencesGroup::builder()
            .title("Recording")
            .build();

        let ptt_row = ActionRow::builder()
            .title("Push to Talk")
            .subtitle("Hold shortcut to record")
            .build();
        let ptt_switch = Switch::builder()
            .active(state.settings.push_to_talk())
            .build();
        ptt_row.add_suffix(&ptt_switch);
        ptt_switch.connect_active_notify({
            let settings = state.settings.clone();
            move |switch| {
                settings.set_push_to_talk(switch.is_active());
            }
        });
        recording_group.add(&ptt_row);

        let mic_row = ActionRow::builder()
            .title("Microphone")
            .subtitle("Select input device")
            .build();
        recording_group.add(&mic_row);

        let mute_row = ActionRow::builder()
            .title("Mute While Recording")
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

        vbox.append(&recording_group);

        let audio_feedback_group = PreferencesGroup::builder()
            .title("Audio Feedback")
            .build();

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

        let volume_row = ActionRow::builder()
            .title("Volume")
            .build();
        let volume_scale = gtk::Scale::builder()
            .adjustment(&gtk::Adjustment::new(
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

        let output_row = ActionRow::builder()
            .title("Output Device")
            .subtitle("Select audio output device")
            .build();
        audio_feedback_group.add(&output_row);

        vbox.append(&audio_feedback_group);

        let language_group = PreferencesGroup::builder()
            .title("Language")
            .build();

        let lang_row = ActionRow::builder()
            .title("Transcription Language")
            .subtitle("Language to transcribe")
            .build();
        language_group.add(&lang_row);

        let translate_row = ActionRow::builder()
            .title("Translate to English")
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

        vbox.append(&language_group);

        container.set_child(Some(&vbox));

        Self { container }
    }
}

impl Page for GeneralPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

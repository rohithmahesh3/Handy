use gtk::prelude::*;
use gtk::{Widget, ScrolledWindow, Box, Orientation, Label, Switch, ComboBoxText, Scale, Button};
use libadwaita::{PreferencesGroup, ActionRow, ExpanderRow};
use std::sync::Arc;

use crate::app::AppState;
use crate::settings::{ModelUnloadTimeout, PasteMethod, TypingTool};
use super::Page;

pub struct AdvancedPage {
    container: ScrolledWindow,
}

impl AdvancedPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let main_box = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let language_group = PreferencesGroup::builder()
            .title("Language")
            .build();

        let language_row = ActionRow::builder()
            .title("Transcription Language")
            .subtitle("Language for transcription (auto = detect)")
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
                state_clone.settings.set_selected_language(active);
            }
        });
        language_row.add_suffix(&language_combo);
        language_group.add(&language_row);

        let translate_row = ActionRow::builder()
            .title("Translate to English")
            .subtitle("Translate non-English speech to English")
            .build();

        let translate_switch = Switch::builder()
            .active(state.settings.translate_to_english())
            .build();
        
        let state_clone = state.clone();
        translate_switch.connect_active_notify(move |switch| {
            state_clone.settings.set_translate_to_english(switch.is_active());
        });
        translate_row.add_suffix(&translate_switch);
        language_group.add(&translate_row);

        main_box.append(&language_group);

        let model_group = PreferencesGroup::builder()
            .title("Model")
            .build();

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
                        state_clone.settings.set_model_unload_timeout(timeouts[idx].0);
                    }
                }
            }
        });
        timeout_row.add_suffix(&timeout_combo);
        model_group.add(&timeout_row);

        main_box.append(&model_group);

        let output_group = PreferencesGroup::builder()
            .title("Output")
            .build();

        let paste_method_row = ActionRow::builder()
            .title("Paste Method")
            .subtitle("How to insert transcribed text")
            .build();

        let paste_combo = ComboBoxText::new();
        let paste_methods = [
            (PasteMethod::Direct, "Type directly"),
            (PasteMethod::CtrlV, "Ctrl+V"),
            (PasteMethod::CtrlShiftV, "Ctrl+Shift+V"),
            (PasteMethod::ShiftInsert, "Shift+Insert"),
            (PasteMethod::None, "No paste (clipboard only)"),
        ];

        let current_paste = state.settings.paste_method();
        let mut paste_index = 0;
        for (i, (method, name)) in paste_methods.iter().enumerate() {
            paste_combo.append(Some(&format!("{}", i)), name);
            if *method == current_paste {
                paste_index = i as u32;
            }
        }
        paste_combo.set_active(Some(paste_index));

        let state_clone = state.clone();
        paste_combo.connect_changed(move |combo| {
            if let Some(id) = combo.active_id() {
                if let Ok(idx) = id.parse::<usize>() {
                    if idx < paste_methods.len() {
                        state_clone.settings.set_paste_method(paste_methods[idx].0);
                    }
                }
            }
        });
        paste_method_row.add_suffix(&paste_combo);
        output_group.add(&paste_method_row);

        let typing_tool_row = ActionRow::builder()
            .title("Typing Tool")
            .subtitle("Tool for typing text (wtype recommended)")
            .build();

        let typing_combo = ComboBoxText::new();
        let typing_tools = [
            (TypingTool::Auto, "Auto"),
            (TypingTool::Wtype, "wtype"),
            (TypingTool::Kwtype, "kwtype"),
            (TypingTool::Dotool, "dotool"),
            (TypingTool::Ydotool, "ydotool"),
        ];

        let current_tool = state.settings.typing_tool();
        let mut tool_index = 0;
        for (i, (tool, name)) in typing_tools.iter().enumerate() {
            typing_combo.append(Some(&format!("{}", i)), name);
            if *tool == current_tool {
                tool_index = i as u32;
            }
        }
        typing_combo.set_active(Some(tool_index));

        let state_clone = state.clone();
        typing_combo.connect_changed(move |combo| {
            if let Some(id) = combo.active_id() {
                if let Ok(idx) = id.parse::<usize>() {
                    if idx < typing_tools.len() {
                        state_clone.settings.set_typing_tool(typing_tools[idx].0);
                    }
                }
            }
        });
        typing_tool_row.add_suffix(&typing_combo);
        output_group.add(&typing_tool_row);

        let space_row = ActionRow::builder()
            .title("Append Trailing Space")
            .subtitle("Add space after each transcription")
            .build();

        let space_switch = Switch::builder()
            .active(state.settings.append_trailing_space())
            .build();
        
        let state_clone = state.clone();
        space_switch.connect_active_notify(move |switch| {
            state_clone.settings.set_append_trailing_space(switch.is_active());
        });
        space_row.add_suffix(&space_switch);
        output_group.add(&space_row);

        main_box.append(&output_group);

        let debug_group = PreferencesGroup::builder()
            .title("Debug")
            .build();

        let debug_row = ActionRow::builder()
            .title("Debug Mode")
            .subtitle("Enable verbose logging")
            .build();

        let debug_switch = Switch::builder()
            .active(state.settings.debug_mode())
            .build();
        
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
        
        let state_clone = state.clone();
        experimental_switch.connect_active_notify(move |switch| {
            state_clone.settings.set_experimental_enabled(switch.is_active());
        });
        experimental_row.add_suffix(&experimental_switch);
        debug_group.add(&experimental_row);

        main_box.append(&debug_group);

        let container = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&main_box)
            .build();

        Self { container }
    }
}

impl Page for AdvancedPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

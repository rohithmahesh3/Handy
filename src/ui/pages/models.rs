use gtk::prelude::*;
use gtk::{Widget, ScrolledWindow, Box, Orientation, Button, Label, ProgressBar};
use libadwaita::{PreferencesGroup, ActionRow, ToastOverlay, Toast};
use std::sync::Arc;

use crate::app::AppState;
use crate::managers::model::ModelInfo;
use super::Page;

pub struct ModelsPage {
    container: ScrolledWindow,
}

impl ModelsPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let toast_overlay = ToastOverlay::new();
        
        let main_box = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let models_group = PreferencesGroup::builder()
            .title("Available Models")
            .description("Download and select transcription models")
            .build();

        let models = state.model_manager.get_available_models();
        let selected_model = state.settings.selected_model();

        for model in models {
            let row = create_model_row(&model, &selected_model, state, &toast_overlay);
            models_group.add(&row);
        }

        main_box.append(&models_group);

        let custom_group = PreferencesGroup::builder()
            .title("Custom Models")
            .description("Place Whisper .bin files in ~/.local/share/handy/models/")
            .build();

        let info_label = Label::builder()
            .label("Custom models are automatically discovered and added to the list above.")
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .margin_top(12)
            .build();
        custom_group.add(&info_label);

        main_box.append(&custom_group);

        toast_overlay.set_child(Some(&main_box));

        let container = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&toast_overlay)
            .build();

        Self { container }
    }
}

fn create_model_row(
    model: &ModelInfo,
    selected_model: &str,
    state: &Arc<AppState>,
    _toast_overlay: &ToastOverlay,
) -> ActionRow {
    let row = ActionRow::builder()
        .title(&model.name)
        .subtitle(&model.description)
        .build();

    if model.is_recommended {
        row.add_prefix(&gtk::Image::from_icon_name("starred-symbolic"));
    }

    let size_label = Label::builder()
        .label(format!("{} MB", model.size_mb))
        .css_classes(["dim-label", "caption"])
        .build();
    row.add_suffix(&size_label);

    let state_box = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();

    if model.is_downloaded {
        if model.id == selected_model {
            let active_label = Label::builder()
                .label("Active")
                .css_classes(["success", "caption"])
                .build();
            state_box.append(&active_label);
        } else {
            let select_btn = Button::builder()
                .label("Select")
                .css_classes(["pill", "suggested-action"])
                .build();
            
            let model_id = model.id.clone();
            let state_clone = state.clone();
            select_btn.connect_clicked(move |_| {
                if let Err(e) = state_clone.model_manager.set_active_model(&model_id) {
                    log::error!("Failed to set active model: {}", e);
                }
            });
            state_box.append(&select_btn);
        }

        if !model.is_custom {
            let delete_btn = Button::builder()
                .icon_name("user-trash-symbolic")
                .css_classes(["destructive-action", "pill"])
                .build();
            
            let model_id = model.id.clone();
            let state_clone = state.clone();
            delete_btn.connect_clicked(move |_| {
                if let Err(e) = state_clone.model_manager.delete_model(&model_id) {
                    log::error!("Failed to delete model: {}", e);
                }
            });
            state_box.append(&delete_btn);
        }
    } else if model.is_downloading {
        let progress = ProgressBar::builder()
            .fraction(0.5)
            .show_text(true)
            .text("Downloading...")
            .width_request(100)
            .build();
        state_box.append(&progress);
    } else if model.url.is_some() {
        let download_btn = Button::builder()
            .label("Download")
            .css_classes(["pill", "suggested-action"])
            .build();
        
        let model_id = model.id.clone();
        let state_clone = state.clone();
        download_btn.connect_clicked(move |_| {
            let model_id = model_id.clone();
            let state = state_clone.clone();
            tokio::spawn(async move {
                if let Err(e) = state.model_manager.download_model(&model_id).await {
                    log::error!("Failed to download model: {}", e);
                }
            });
        });
        state_box.append(&download_btn);
    }

    row.add_suffix(&state_box);
    row
}

impl Page for ModelsPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}

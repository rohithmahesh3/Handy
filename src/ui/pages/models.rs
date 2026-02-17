use gtk4::prelude::*;
use gtk4::{
    Box, Button, Image, Label, Orientation, PolicyType, ProgressBar, ScrolledWindow, Widget,
};
use libadwaita::prelude::{ActionRowExt, PreferencesGroupExt};
use libadwaita::{ActionRow, Clamp, PreferencesGroup, ToastOverlay};
use std::sync::Arc;

use super::Page;
use crate::app::AppState;
use crate::managers::model::ModelInfo;

pub struct ModelsPage {
    container: ScrolledWindow,
}

impl ModelsPage {
    pub fn new(state: &Arc<AppState>) -> Self {
        let toast_overlay = ToastOverlay::new();

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

        let models_group = PreferencesGroup::builder()
            .title("Available Models")
            .description("Download and select transcription models")
            .build();

        let selected_model = state.model_manager.get_current_model();
        for model in sorted_models(state) {
            let row = create_model_row(&model, &selected_model, state);
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

        let clamp = Clamp::builder()
            .maximum_size(900)
            .tightening_threshold(600)
            .child(&toast_overlay)
            .build();

        let container = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .child(&clamp)
            .build();

        Self { container }
    }
}

fn sorted_models(state: &Arc<AppState>) -> Vec<ModelInfo> {
    let mut models = state.model_manager.get_available_models();
    models.sort_by(|a, b| {
        b.is_recommended
            .cmp(&a.is_recommended)
            .then_with(|| b.is_downloaded.cmp(&a.is_downloaded))
            .then_with(|| a.name.cmp(&b.name))
    });
    models
}

fn create_model_row(model: &ModelInfo, selected_model: &str, state: &Arc<AppState>) -> ActionRow {
    let row = ActionRow::builder()
        .title(&model.name)
        .subtitle(&model.description)
        .build();

    if model.is_recommended {
        row.add_prefix(&Image::from_icon_name("starred-symbolic"));
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
        let total_bytes = model.size_mb.saturating_mul(1024 * 1024);
        let fraction = if total_bytes == 0 {
            0.0
        } else {
            (model.partial_size as f64 / total_bytes as f64).clamp(0.0, 1.0)
        };
        let progress_text = format!("{:.0}%", fraction * 100.0);

        let progress = ProgressBar::builder()
            .fraction(fraction)
            .show_text(true)
            .text(&progress_text)
            .width_request(120)
            .build();
        state_box.append(&progress);

        let cancel_btn = Button::builder()
            .label("Cancel")
            .css_classes(["pill"])
            .build();
        let model_id = model.id.clone();
        let state_clone = state.clone();
        cancel_btn.connect_clicked(move |_| {
            if let Err(e) = state_clone.model_manager.cancel_download(&model_id) {
                log::error!("Failed to cancel download: {}", e);
            }
        });
        state_box.append(&cancel_btn);
    } else if model.url.is_some() {
        let download_btn = Button::builder()
            .label("Download")
            .css_classes(["pill", "suggested-action"])
            .build();

        let model_id = model.id.clone();
        let model_manager = state.model_manager.clone();
        download_btn.connect_clicked(move |_| {
            let model_id = model_id.clone();
            let model_manager = model_manager.clone();
            std::thread::spawn(move || {
                let runtime = match tokio::runtime::Runtime::new() {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        log::error!("Failed to create Tokio runtime for model download: {}", e);
                        return;
                    }
                };

                if let Err(e) = runtime.block_on(model_manager.download_model(&model_id)) {
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

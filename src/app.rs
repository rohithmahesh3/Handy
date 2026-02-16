use gtk::prelude::*;
use libadwaita::Application as AdwApplication;
use std::sync::Arc;

use crate::dbus::{self, HandyState};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::Settings;
use crate::ui::window::MainWindow;

const APP_ID: &str = "com.handy.Transcription";

pub struct AppState {
    pub settings: Settings,
    pub recording_manager: Arc<AudioRecordingManager>,
    pub model_manager: Arc<ModelManager>,
    pub transcription_manager: Arc<TranscriptionManager>,
}

pub fn run_app() {
    gtk::init().expect("Failed to initialize GTK");
    libadwaita::init();

    let settings = Settings::new();

    let recording_manager = Arc::new(
        AudioRecordingManager::new().expect("Failed to initialize recording manager"),
    );
    let model_manager = Arc::new(
        ModelManager::new().expect("Failed to initialize model manager"),
    );
    let transcription_manager = Arc::new(
        TranscriptionManager::new(model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );

    let state = Arc::new(AppState {
        settings: settings.clone(),
        recording_manager: recording_manager.clone(),
        model_manager: model_manager.clone(),
        transcription_manager: transcription_manager.clone(),
    });

    let handy_state = Arc::new(HandyState::new(
        settings,
        recording_manager,
        transcription_manager,
    ));

    let dbus_state = handy_state.clone();
    glib::MainContext::default().spawn_local(async move {
        match dbus::start_dbus_server(dbus_state).await {
            Ok(_) => log::info!("D-Bus server started successfully"),
            Err(e) => log::error!("Failed to start D-Bus server: {}", e),
        }
    });

    let app = AdwApplication::builder()
        .application_id(APP_ID)
        .build();

    let state_clone = state.clone();
    app.connect_activate(move |app| {
        let main_window = MainWindow::new(app, state_clone.clone());
        main_window.present();
    });

    app.run();
}

use gtk4::prelude::*;
use libadwaita::Application as AdwApplication;
use std::sync::Arc;

use crate::dbus::{self, HandyState};
use crate::global_shortcuts::start_global_shortcuts_listener;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::Settings;
use crate::ui::window::MainWindow;

const UI_APP_ID: &str = "com.handy.Handy";

pub struct AppState {
    pub settings: Settings,
    pub model_manager: Arc<ModelManager>,
}

struct RuntimeState {
    settings: Settings,
    recording_manager: Arc<AudioRecordingManager>,
    model_manager: Arc<ModelManager>,
    transcription_manager: Arc<TranscriptionManager>,
}

fn init_ui_state() -> Arc<AppState> {
    let settings = Settings::new();
    let model_manager = Arc::new(ModelManager::new().expect("Failed to initialize model manager"));

    #[allow(clippy::arc_with_non_send_sync)]
    Arc::new(AppState {
        settings,
        model_manager,
    })
}

fn init_runtime() -> (Arc<RuntimeState>, Arc<HandyState>) {
    let settings = Settings::new();

    let recording_manager =
        Arc::new(AudioRecordingManager::new().expect("Failed to initialize recording manager"));
    let model_manager = Arc::new(ModelManager::new().expect("Failed to initialize model manager"));
    let transcription_manager = Arc::new(
        TranscriptionManager::new(model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );

    #[allow(clippy::arc_with_non_send_sync)]
    let state = Arc::new(RuntimeState {
        settings: settings.clone(),
        recording_manager: recording_manager.clone(),
        model_manager: model_manager.clone(),
        transcription_manager: transcription_manager.clone(),
    });

    let handy_state = Arc::new(HandyState::new(
        recording_manager,
        transcription_manager,
        settings.selected_language(),
    ));

    wire_settings_sync(&state, &handy_state);

    (state, handy_state)
}

fn wire_settings_sync(state: &Arc<RuntimeState>, handy_state: &Arc<HandyState>) {
    state.settings.connect_changed(Some("selected-language"), {
        let settings = state.settings.clone();
        let handy_state = handy_state.clone();
        let tm = state.transcription_manager.clone();
        move |_| {
            *handy_state.selected_language.lock().unwrap() = settings.selected_language();
            tm.refresh_config_from_settings(&settings);
        }
    });

    state
        .settings
        .connect_changed(Some("translate-to-english"), {
            let settings = state.settings.clone();
            let tm = state.transcription_manager.clone();
            move |_| {
                tm.refresh_config_from_settings(&settings);
            }
        });

    state.settings.connect_changed(Some("custom-words"), {
        let settings = state.settings.clone();
        let tm = state.transcription_manager.clone();
        move |_| {
            tm.refresh_config_from_settings(&settings);
        }
    });

    state
        .settings
        .connect_changed(Some("word-correction-threshold"), {
            let settings = state.settings.clone();
            let tm = state.transcription_manager.clone();
            move |_| {
                tm.refresh_config_from_settings(&settings);
            }
        });

    state
        .settings
        .connect_changed(Some("model-unload-timeout"), {
            let settings = state.settings.clone();
            let tm = state.transcription_manager.clone();
            move |_| {
                tm.refresh_config_from_settings(&settings);
            }
        });

    state.settings.connect_changed(Some("selected-model"), {
        let settings = state.settings.clone();
        let model_manager = state.model_manager.clone();
        let tm = state.transcription_manager.clone();
        move |_| {
            if let Err(e) = model_manager.sync_selected_model_from_settings() {
                log::error!("Failed to sync selected model from settings: {}", e);
            }
            if let Err(e) = tm.unload_model() {
                log::error!("Failed to unload model after model selection change: {}", e);
            }
            tm.refresh_config_from_settings(&settings);
        }
    });

    state
        .settings
        .connect_changed(Some("mute-while-recording"), {
            let settings = state.settings.clone();
            let recording_manager = state.recording_manager.clone();
            move |_| {
                recording_manager.set_mute_while_recording(settings.mute_while_recording());
            }
        });

    state
        .settings
        .connect_changed(Some("selected-microphone"), {
            let settings = state.settings.clone();
            let recording_manager = state.recording_manager.clone();
            move |_| {
                if let Err(e) =
                    recording_manager.set_selected_microphone(settings.selected_microphone())
                {
                    log::error!("Failed to switch microphone: {}", e);
                }
            }
        });

    state
        .settings
        .connect_changed(Some("always-on-microphone"), {
            let settings = state.settings.clone();
            let recording_manager = state.recording_manager.clone();
            move |_| {
                if let Err(e) =
                    recording_manager.set_mode_from_settings(settings.always_on_microphone())
                {
                    log::error!("Failed to switch microphone mode: {}", e);
                }
            }
        });

    // Additional settings listeners for live updates
    state.settings.connect_changed(Some("audio-feedback"), {
        let settings = state.settings.clone();
        move |_| {
            let enabled = settings.audio_feedback();
            log::info!("Audio feedback setting changed to: {}", enabled);
            // Audio feedback is read on-demand during playback
        }
    });

    state
        .settings
        .connect_changed(Some("audio-feedback-volume"), {
            let settings = state.settings.clone();
            move |_| {
                let volume = settings.audio_feedback_volume();
                log::info!("Audio feedback volume changed to: {}", volume);
                // Audio feedback volume is read on-demand during playback
            }
        });

    state.settings.connect_changed(Some("sound-theme"), {
        let settings = state.settings.clone();
        move |_| {
            let theme = settings.sound_theme();
            log::info!("Sound theme changed to: {:?}", theme);
            // Sound theme is read on-demand during playback
        }
    });

    state.settings.connect_changed(Some("debug-mode"), {
        move |_| {
            log::info!("Debug mode setting changed - restart required for full effect");
        }
    });

    state
        .settings
        .connect_changed(Some("experimental-enabled"), {
            move |_| {
                log::info!(
                    "Experimental features setting changed - restart required for full effect"
                );
            }
        });
}

pub fn run_ui() {
    gtk4::init().expect("Failed to initialize GTK");
    let _ = libadwaita::init();

    let state = init_ui_state();

    let app = AdwApplication::builder().application_id(UI_APP_ID).build();

    let state_clone = state.clone();
    app.connect_activate(move |app| {
        let main_window = MainWindow::new(app, state_clone.clone());
        main_window.present();
    });

    app.run();
}

pub fn run_daemon() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let (_runtime_state, handy_state) = init_runtime();

    let context = glib::MainContext::default();
    match context.block_on(dbus::start_dbus_server(handy_state)) {
        Ok(dbus_state) => {
            start_global_shortcuts_listener();
            let main_loop = glib::MainLoop::new(None, false);

            // Setup shutdown signal handler
            let shutdown_requested = Arc::new(AtomicBool::new(false));
            let shutdown_flag = shutdown_requested.clone();

            if let Err(e) = ctrlc::set_handler(move || {
                log::info!("Shutdown signal received, initiating graceful shutdown...");
                shutdown_flag.store(true, Ordering::SeqCst);
            }) {
                log::error!("Failed to set Ctrl-C handler: {}", e);
            }

            // Monitor shutdown flag
            let main_loop_clone = main_loop.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                if shutdown_requested.load(Ordering::SeqCst) {
                    main_loop_clone.quit();
                }
                glib::ControlFlow::Continue
            });

            main_loop.run();

            // Graceful shutdown
            log::info!("Shutting down D-Bus server...");
            if let Err(e) = context.block_on(dbus::stop_dbus_server(&dbus_state)) {
                log::error!("Error during D-Bus server shutdown: {}", e);
            }
            log::info!("Shutdown complete");
        }
        Err(e) => {
            eprintln!("Failed to start D-Bus server in daemon mode: {}", e);
            std::process::exit(1);
        }
    }
}

//! D-Bus server for IBus integration
//!
//! This module provides a D-Bus interface that allows the handy-ibus engine
//! to control Handy's transcription functionality.
//!
//! # D-Bus Interface
//!
//! **Bus Name:** `com.handy.Transcription`
//! **Object Path:** `/com/handy/Transcription`
//!
//! ## Methods
//! - `StartRecording()` - Start recording audio
//! - `StopRecording()` → `s` (text) - Stop recording and return transcribed text
//! - `CancelRecording()` - Cancel current recording without transcription
//! - `GetState()` → `(bb)` - Returns (is_recording, is_model_loaded)
//!
//! ## Signals
//! - `TranscriptionReady(s: text)` - Emitted when transcription is complete
//! - `RecordingStateChanged(b: is_recording)` - Emitted when recording state changes
//! - `Error(s: message)` - Emitted when an error occurs

use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::get_settings;
use crate::tray::{change_tray_icon, TrayIconState};
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::Connection;

/// State for the D-Bus server
pub struct HandyDbusState {
    /// Whether the D-Bus server is running
    running: AtomicBool,
    /// The D-Bus connection handle
    connection: std::sync::Mutex<Option<Connection>>,
}

impl HandyDbusState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            connection: std::sync::Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// The D-Bus interface for Handy transcription
struct HandyTranscription {
    app: AppHandle,
}

#[zbus::interface(name = "com.handy.Transcription")]
impl HandyTranscription {
    /// Start recording audio
    async fn start_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: StartRecording called");
        let start_time = Instant::now();

        let tm = self.app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();

        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        let settings = get_settings(&self.app);

        change_tray_icon(&self.app, TrayIconState::Recording);

        let is_always_on = settings.always_on_microphone;
        let recording_started = if is_always_on {
            let rm_clone = Arc::clone(&rm);
            let app_clone = self.app.clone();
            std::thread::spawn(move || {
                crate::audio_feedback::play_feedback_sound_blocking(
                    &app_clone,
                    crate::audio_feedback::SoundType::Start,
                );
                rm_clone.apply_mute();
            });
            rm.try_start_recording("ibus")
        } else {
            if rm.try_start_recording("ibus") {
                let app_clone = self.app.clone();
                let rm_clone = Arc::clone(&rm);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    crate::audio_feedback::play_feedback_sound_blocking(
                        &app_clone,
                        crate::audio_feedback::SoundType::Start,
                    );
                    rm_clone.apply_mute();
                });
                true
            } else {
                false
            }
        };

        if recording_started {
            self.emit_recording_state_changed(true).await?;
            info!("D-Bus: Recording started in {:?}", start_time.elapsed());
            Ok(())
        } else {
            change_tray_icon(&self.app, TrayIconState::Idle);
            Err(fdo::Error::Failed("Failed to start recording".to_string()))
        }
    }

    /// Stop recording and return transcribed text
    async fn stop_recording(&self) -> fdo::Result<String> {
        debug!("D-Bus: StopRecording called");
        let stop_time = Instant::now();

        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        let tm = self.app.state::<Arc<TranscriptionManager>>();
        let hm = self.app.state::<Arc<HistoryManager>>();

        change_tray_icon(&self.app, TrayIconState::Transcribing);

        rm.remove_mute();
        crate::audio_feedback::play_feedback_sound(
            &self.app,
            crate::audio_feedback::SoundType::Stop,
        );

        if let Some(samples) = rm.stop_recording("ibus") {
            debug!(
                "D-Bus: Recording stopped, {} samples retrieved in {:?}",
                samples.len(),
                stop_time.elapsed()
            );

            let transcription_time = Instant::now();
            match tm.transcribe(samples.clone()) {
                Ok(transcription) => {
                    debug!(
                        "D-Bus: Transcription completed in {:?}: '{}'",
                        transcription_time.elapsed(),
                        transcription
                    );

                    let settings = get_settings(&self.app);
                    let mut final_text = transcription.clone();

                    // Handle Chinese variant conversion
                    let is_simplified = settings.selected_language == "zh-Hans";
                    let is_traditional = settings.selected_language == "zh-Hant";

                    if is_simplified || is_traditional {
                        let config = if is_simplified {
                            ferrous_opencc::config::BuiltinConfig::Tw2sp
                        } else {
                            ferrous_opencc::config::BuiltinConfig::S2twp
                        };

                        if let Ok(converter) = ferrous_opencc::OpenCC::from_config(config) {
                            final_text = converter.convert(&transcription);
                        }
                    }

                    // Save to history
                    let hm_clone = Arc::clone(&hm);
                    let transcription_for_history = transcription.clone();
                    let samples_for_history = samples;
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = hm_clone
                            .save_transcription(
                                samples_for_history,
                                transcription_for_history,
                                None,
                                None,
                            )
                            .await
                        {
                            error!("Failed to save transcription to history: {}", e);
                        }
                    });

                    self.emit_recording_state_changed(false).await?;
                    change_tray_icon(&self.app, TrayIconState::Idle);

                    // Emit the transcription ready signal
                    self.emit_transcription_ready(&final_text).await?;

                    Ok(final_text)
                }
                Err(err) => {
                    error!("D-Bus: Transcription error: {}", err);
                    self.emit_recording_state_changed(false).await?;
                    change_tray_icon(&self.app, TrayIconState::Idle);
                    Err(fdo::Error::Failed(format!("Transcription failed: {}", err)))
                }
            }
        } else {
            warn!("D-Bus: No samples retrieved from recording stop");
            self.emit_recording_state_changed(false).await?;
            change_tray_icon(&self.app, TrayIconState::Idle);
            Ok(String::new())
        }
    }

    /// Cancel current recording without transcription
    async fn cancel_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: CancelRecording called");

        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        rm.remove_mute();
        rm.cancel_recording();

        crate::audio_feedback::play_feedback_sound(
            &self.app,
            crate::audio_feedback::SoundType::Stop,
        );

        self.emit_recording_state_changed(false).await?;
        change_tray_icon(&self.app, TrayIconState::Idle);

        info!("D-Bus: Recording cancelled");
        Ok(())
    }

    /// Get current state: (is_recording, is_model_loaded)
    async fn get_state(&self) -> fdo::Result<(bool, bool)> {
        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        let tm = self.app.state::<Arc<TranscriptionManager>>();

        let is_recording = rm.is_recording();
        let is_model_loaded = tm.is_model_loaded();

        Ok((is_recording, is_model_loaded))
    }

    /// Get the currently selected language
    async fn get_language(&self) -> fdo::Result<String> {
        let settings = get_settings(&self.app);
        Ok(settings.selected_language.clone())
    }

    /// Set the language for transcription
    async fn set_language(&self, language: String) -> fdo::Result<()> {
        let mut settings = get_settings(&self.app);
        settings.selected_language = language;
        crate::settings::write_settings(&self.app, settings);
        Ok(())
    }

    /// Signal emitted when transcription is ready
    #[zbus(signal)]
    async fn transcription_ready(ctxt: &SignalEmitter<'_>, text: &str) -> zbus::Result<()>;

    /// Signal emitted when recording state changes
    #[zbus(signal)]
    async fn recording_state_changed(
        ctxt: &SignalEmitter<'_>,
        is_recording: bool,
    ) -> zbus::Result<()>;

    /// Signal emitted when an error occurs
    #[zbus(signal)]
    async fn error(ctxt: &SignalEmitter<'_>, message: &str) -> zbus::Result<()>;
}

impl HandyTranscription {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }

    async fn emit_transcription_ready(&self, text: &str) -> fdo::Result<()> {
        let connection = self
            .app
            .try_state::<HandyDbusState>()
            .and_then(|state| state.connection.lock().ok().and_then(|c| c.clone()));

        if let Some(conn) = connection {
            if let Err(e) = Self::transcription_ready(&conn.object_server(), text).await {
                error!("Failed to emit TranscriptionReady signal: {}", e);
            }
        }
        Ok(())
    }

    async fn emit_recording_state_changed(&self, is_recording: bool) -> fdo::Result<()> {
        let connection = self
            .app
            .try_state::<HandyDbusState>()
            .and_then(|state| state.connection.lock().ok().and_then(|c| c.clone()));

        if let Some(conn) = connection {
            if let Err(e) = Self::recording_state_changed(&conn.object_server(), is_recording).await
            {
                error!("Failed to emit RecordingStateChanged signal: {}", e);
            }
        }
        Ok(())
    }
}

/// Start the D-Bus server
pub async fn start_dbus_server(app: &AppHandle) -> Result<(), String> {
    info!("Starting D-Bus server for IBus integration...");

    let dbus_state = app.state::<HandyDbusState>();

    if dbus_state.is_running() {
        warn!("D-Bus server is already running");
        return Ok(());
    }

    let app_clone = app.clone();

    // Build the connection
    let connection = Connection::session()
        .await
        .map_err(|e| format!("Failed to connect to session bus: {}", e))?;

    // Request the bus name
    connection
        .request_name("com.handy.Transcription")
        .await
        .map_err(|e| format!("Failed to request bus name: {}", e))?;

    // Create the interface
    let transcription = HandyTranscription::new(app_clone);

    // Register the object
    connection
        .object_server()
        .at("/com/handy/Transcription", transcription)
        .await
        .map_err(|e| format!("Failed to register D-Bus object: {}", e))?;

    // Store the connection
    {
        let mut conn_guard = dbus_state
            .connection
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        *conn_guard = Some(connection);
    }

    dbus_state.running.store(true, Ordering::SeqCst);

    info!("D-Bus server started successfully on com.handy.Transcription");
    Ok(())
}

/// Stop the D-Bus server
pub async fn stop_dbus_server(app: &AppHandle) -> Result<(), String> {
    info!("Stopping D-Bus server...");

    let dbus_state = app.state::<HandyDbusState>();

    if !dbus_state.is_running() {
        return Ok(());
    }

    // Clear the connection (this will drop it and release the bus name)
    {
        let mut conn_guard = dbus_state
            .connection
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        *conn_guard = None;
    }

    dbus_state.running.store(false, Ordering::SeqCst);

    info!("D-Bus server stopped");
    Ok(())
}

//! D-Bus server for IBus integration
//!
//! This module provides a D-Bus interface that allows the handy-ibus engine
//! to control Handy's transcription functionality.

use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::Settings;
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::Connection;

/// Shared state for the D-Bus server and handlers
pub struct HandyState {
    pub settings: Settings,
    pub recording_manager: Arc<AudioRecordingManager>,
    pub transcription_manager: Arc<TranscriptionManager>,
    pub history_manager: Arc<HistoryManager>,
    pub is_recording: AtomicBool,
}

impl HandyState {
    pub fn new(
        settings: Settings,
        recording_manager: Arc<AudioRecordingManager>,
        transcription_manager: Arc<TranscriptionManager>,
        history_manager: Arc<HistoryManager>,
    ) -> Self {
        Self {
            settings,
            recording_manager,
            transcription_manager,
            history_manager,
            is_recording: AtomicBool::new(false),
        }
    }
}

/// D-Bus state for connection management
pub struct HandyDbusState {
    running: AtomicBool,
    connection: Mutex<Option<Connection>>,
}

impl HandyDbusState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            connection: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// The D-Bus interface for Handy transcription
struct HandyTranscription {
    state: Arc<HandyState>,
    dbus_state: Arc<HandyDbusState>,
}

#[zbus::interface(name = "com.handy.Transcription")]
impl HandyTranscription {
    /// Start recording audio
    async fn start_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: StartRecording called");
        let start_time = Instant::now();

        self.state.transcription_manager.initiate_model_load();

        let is_always_on = self.state.settings.always_on_microphone();
        let recording_started = if is_always_on {
            self.state.recording_manager.try_start_recording("ibus")
        } else {
            if self.state.recording_manager.try_start_recording("ibus") {
                std::thread::spawn({
                    let rm = self.state.recording_manager.clone();
                    move || {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        rm.apply_mute();
                    }
                });
                true
            } else {
                false
            }
        };

        if recording_started {
            self.state.is_recording.store(true, Ordering::SeqCst);
            self.emit_recording_state_changed(true).await?;
            info!("D-Bus: Recording started in {:?}", start_time.elapsed());
            Ok(())
        } else {
            Err(fdo::Error::Failed("Failed to start recording".to_string()))
        }
    }

    /// Stop recording and return transcribed text
    async fn stop_recording(&self) -> fdo::Result<String> {
        debug!("D-Bus: StopRecording called");
        let stop_time = Instant::now();

        self.state.recording_manager.remove_mute();

        if let Some(samples) = self.state.recording_manager.stop_recording("ibus") {
            debug!(
                "D-Bus: Recording stopped, {} samples retrieved in {:?}",
                samples.len(),
                stop_time.elapsed()
            );

            let transcription_time = Instant::now();
            match self.state.transcription_manager.transcribe(samples.clone()) {
                Ok(transcription) => {
                    debug!(
                        "D-Bus: Transcription completed in {:?}: '{}'",
                        transcription_time.elapsed(),
                        transcription
                    );

                    let mut final_text = transcription.clone();

                    // Handle Chinese variant conversion
                    let lang = self.state.settings.selected_language();
                    let is_simplified = lang == "zh-Hans";
                    let is_traditional = lang == "zh-Hant";

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
                    let hm = self.state.history_manager.clone();
                    let transcription_for_history = transcription.clone();
                    let samples_for_history = samples;
                    tokio::spawn(async move {
                        if let Err(e) = hm
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

                    self.state.is_recording.store(false, Ordering::SeqCst);
                    self.emit_recording_state_changed(false).await?;
                    self.emit_transcription_ready(&final_text).await?;

                    Ok(final_text)
                }
                Err(err) => {
                    error!("D-Bus: Transcription error: {}", err);
                    self.state.is_recording.store(false, Ordering::SeqCst);
                    self.emit_recording_state_changed(false).await?;
                    Err(fdo::Error::Failed(format!("Transcription failed: {}", err)))
                }
            }
        } else {
            warn!("D-Bus: No samples retrieved from recording stop");
            self.state.is_recording.store(false, Ordering::SeqCst);
            self.emit_recording_state_changed(false).await?;
            Ok(String::new())
        }
    }

    /// Cancel current recording without transcription
    async fn cancel_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: CancelRecording called");

        self.state.recording_manager.remove_mute();
        self.state.recording_manager.cancel_recording();

        self.state.is_recording.store(false, Ordering::SeqCst);
        self.emit_recording_state_changed(false).await?;

        info!("D-Bus: Recording cancelled");
        Ok(())
    }

    /// Get current state: (is_recording, is_model_loaded)
    async fn get_state(&self) -> fdo::Result<(bool, bool)> {
        let is_recording = self.state.is_recording.load(Ordering::SeqCst);
        let is_model_loaded = self.state.transcription_manager.is_model_loaded();

        Ok((is_recording, is_model_loaded))
    }

    /// Get the currently selected language
    async fn get_language(&self) -> fdo::Result<String> {
        Ok(self.state.settings.selected_language())
    }

    /// Set the language for transcription
    async fn set_language(&self, language: String) -> fdo::Result<()> {
        self.state.settings.set_selected_language(&language);
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
    fn new(state: Arc<HandyState>, dbus_state: Arc<HandyDbusState>) -> Self {
        Self { state, dbus_state }
    }

    async fn emit_transcription_ready(&self, text: &str) -> fdo::Result<()> {
        if let Some(conn) = self.dbus_state.connection.lock().ok().and_then(|c| c.clone()) {
            if let Err(e) = Self::transcription_ready(&conn.object_server(), text).await {
                error!("Failed to emit TranscriptionReady signal: {}", e);
            }
        }
        Ok(())
    }

    async fn emit_recording_state_changed(&self, is_recording: bool) -> fdo::Result<()> {
        if let Some(conn) = self.dbus_state.connection.lock().ok().and_then(|c| c.clone()) {
            if let Err(e) = Self::recording_state_changed(&conn.object_server(), is_recording).await
            {
                error!("Failed to emit RecordingStateChanged signal: {}", e);
            }
        }
        Ok(())
    }
}

/// Start the D-Bus server
pub async fn start_dbus_server(state: Arc<HandyState>) -> Result<Arc<HandyDbusState>, String> {
    info!("Starting D-Bus server for IBus integration...");

    let dbus_state = Arc::new(HandyDbusState::new());

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
    let transcription = HandyTranscription::new(state, dbus_state.clone());

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
    Ok(dbus_state)
}

/// Stop the D-Bus server
pub async fn stop_dbus_server(dbus_state: &HandyDbusState) -> Result<(), String> {
    info!("Stopping D-Bus server...");

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

//! D-Bus server for IBus integration
//!
//! This module provides a D-Bus interface that allows the handy-ibus engine
//! to control Handy's transcription functionality.

use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{PostProcessProvider, Settings};
use crate::text_utils::convert_chinese_variant;
use crate::{audio_feedback::play_feedback_sound, audio_feedback::SoundType};
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zbus::fdo;
use zbus::object_server::SignalContext;
use zbus::Connection;

/// Shared state for the D-Bus server and handlers
pub struct HandyState {
    pub selected_language: Mutex<String>,
    pub recording_manager: Arc<AudioRecordingManager>,
    pub transcription_manager: Arc<TranscriptionManager>,
    pub is_recording: AtomicBool,
}

impl HandyState {
    pub fn new(
        recording_manager: Arc<AudioRecordingManager>,
        transcription_manager: Arc<TranscriptionManager>,
        selected_language: String,
    ) -> Self {
        Self {
            selected_language: Mutex::new(selected_language),
            recording_manager,
            transcription_manager,
            is_recording: AtomicBool::new(false),
        }
    }
}

/// D-Bus state for connection management
pub struct HandyDbusState {
    running: AtomicBool,
    connection: Mutex<Option<Connection>>,
}

impl Default for HandyDbusState {
    fn default() -> Self {
        Self::new()
    }
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

        // Check if a model is selected and downloaded before starting
        if !self.state.transcription_manager.has_model_selected() {
            self.emit_error(
                "No model selected. Open Handy preferences to download and select a model.",
            )
            .await?;
            return Err(fdo::Error::Failed("No model selected".to_string()));
        }

        self.state.transcription_manager.initiate_model_load();

        let recording_started = self.state.recording_manager.try_start_recording("ibus");
        if recording_started {
            let rm = self.state.recording_manager.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                rm.apply_mute();
            });
        }

        if recording_started {
            self.state.is_recording.store(true, Ordering::SeqCst);
            self.emit_recording_state_changed(true).await?;
            play_feedback_sound(&Settings::new(), SoundType::Start);
            info!("D-Bus: Recording started in {:?}", start_time.elapsed());
            Ok(())
        } else {
            self.emit_error("Failed to start recording").await?;
            Err(fdo::Error::Failed("Failed to start recording".to_string()))
        }
    }

    /// Stop recording and return transcribed text
    async fn stop_recording(&self) -> fdo::Result<String> {
        debug!("D-Bus: StopRecording called");
        let stop_time = Instant::now();

        let was_recording = self.state.is_recording.swap(false, Ordering::SeqCst);
        if was_recording {
            self.emit_recording_state_changed(false).await?;
        }

        play_feedback_sound(&Settings::new(), SoundType::Stop);
        self.state.recording_manager.remove_mute();

        if let Some(samples) = self.state.recording_manager.stop_recording("ibus") {
            debug!(
                "D-Bus: Recording stopped, {} samples retrieved in {:?}",
                samples.len(),
                stop_time.elapsed()
            );

            let transcription_time = Instant::now();
            match self.state.transcription_manager.transcribe(samples) {
                Ok(transcription) => {
                    debug!(
                        "D-Bus: Transcription completed in {:?}: '{}'",
                        transcription_time.elapsed(),
                        transcription
                    );

                    let lang = self.state.selected_language.lock().unwrap().clone();
                    let converted_text = convert_chinese_variant(&transcription, &lang);
                    let output_text =
                        match post_process_transcription_if_enabled(&converted_text).await {
                            Some(text) => text,
                            None => converted_text,
                        };

                    self.emit_transcription_ready(&output_text).await?;

                    Ok(output_text)
                }
                Err(err) => {
                    error!("D-Bus: Transcription error: {}", err);
                    self.emit_error(&format!("Transcription failed: {}", err))
                        .await?;
                    Err(fdo::Error::Failed(format!("Transcription failed: {}", err)))
                }
            }
        } else {
            warn!("D-Bus: No samples retrieved from recording stop");
            Ok(String::new())
        }
    }

    /// Cancel current recording without transcription
    async fn cancel_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: CancelRecording called");

        self.state.recording_manager.cancel_recording();

        self.state.is_recording.store(false, Ordering::SeqCst);
        self.emit_recording_state_changed(false).await?;

        info!("D-Bus: Recording cancelled");
        Ok(())
    }

    /// Get current state: (is_recording, has_model_selected)
    ///
    /// The second value indicates whether a model is selected and downloaded
    /// (not whether it's currently loaded in memory — it may be auto-loaded on demand).
    async fn get_state(&self) -> fdo::Result<(bool, bool)> {
        let is_recording = self.state.is_recording.load(Ordering::SeqCst);
        let has_model = self.state.transcription_manager.has_model_selected();

        Ok((is_recording, has_model))
    }

    /// Get the currently selected language
    async fn get_language(&self) -> fdo::Result<String> {
        Ok(self.state.selected_language.lock().unwrap().clone())
    }

    /// Set the language for transcription
    async fn set_language(&self, language: String) -> fdo::Result<()> {
        *self.state.selected_language.lock().unwrap() = language.clone();
        let settings = Settings::new();
        settings.set_selected_language(&language);
        self.state
            .transcription_manager
            .refresh_config_from_settings(&settings);
        Ok(())
    }

    /// Signal emitted when transcription is ready
    #[zbus(signal)]
    async fn transcription_ready(ctxt: &SignalContext<'_>, text: &str) -> zbus::Result<()>;

    /// Signal emitted when recording state changes
    #[zbus(signal)]
    async fn recording_state_changed(
        ctxt: &SignalContext<'_>,
        is_recording: bool,
    ) -> zbus::Result<()>;

    /// Signal emitted when an error occurs
    #[zbus(signal)]
    async fn error(ctxt: &SignalContext<'_>, message: &str) -> zbus::Result<()>;
}

struct PostProcessRequest {
    provider: PostProcessProvider,
    api_key: String,
    model: String,
    prompt_text: String,
}

fn build_post_process_request(text: &str) -> Option<PostProcessRequest> {
    let settings = Settings::new();
    if !settings.post_process_enabled() {
        return None;
    }

    let provider_id = settings.post_process_provider_id();
    let api_key = settings.post_process_api_keys().get(&provider_id)?.clone();
    if api_key.is_empty() {
        return None;
    }
    let model = settings.post_process_models().get(&provider_id)?.clone();
    if model.is_empty() {
        return None;
    }

    let prompts = settings.post_process_prompts();
    let selected_id = settings.post_process_selected_prompt_id();
    let prompt = if let Some(selected) = selected_id {
        prompts.iter().find(|p| p.id == selected)
    } else {
        prompts.first()
    }?;

    let base_url = settings
        .post_process_base_urls()
        .get(&provider_id)
        .cloned()
        .unwrap_or_else(|| match provider_id.as_str() {
            "openai" => "https://api.openai.com/v1".to_string(),
            "anthropic" => "https://api.anthropic.com/v1".to_string(),
            "openrouter" => "https://openrouter.ai/api/v1".to_string(),
            "groq" => "https://api.groq.com/openai/v1".to_string(),
            "cerebras" => "https://api.cerebras.ai/v1".to_string(),
            _ => "http://localhost:11434/v1".to_string(),
        });
    let provider = PostProcessProvider {
        id: provider_id.clone(),
        label: provider_id.clone(),
        base_url,
        allow_base_url_edit: provider_id == "custom",
    };

    let prompt_text = prompt.prompt.replace("${output}", text);
    Some(PostProcessRequest {
        provider,
        api_key,
        model,
        prompt_text,
    })
}

async fn post_process_transcription_if_enabled(text: &str) -> Option<String> {
    let request = build_post_process_request(text)?;
    let processed = crate::llm_client::send_chat_completion(
        &request.provider,
        request.api_key,
        &request.model,
        request.prompt_text,
    )
    .await
    .ok()
    .flatten()?;
    let trimmed = processed.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

impl HandyTranscription {
    fn new(state: Arc<HandyState>, dbus_state: Arc<HandyDbusState>) -> Self {
        Self { state, dbus_state }
    }

    async fn emit_transcription_ready(&self, text: &str) -> fdo::Result<()> {
        if let Some(conn) = self
            .dbus_state
            .connection
            .lock()
            .ok()
            .and_then(|c| c.clone())
        {
            let iface_ref = conn
                .object_server()
                .interface::<_, Self>("/com/handy/Transcription")
                .await;
            if let Ok(iface_ref) = iface_ref {
                if let Err(e) = Self::transcription_ready(iface_ref.signal_context(), text).await {
                    error!("Failed to emit TranscriptionReady signal: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn emit_recording_state_changed(&self, is_recording: bool) -> fdo::Result<()> {
        if let Some(conn) = self
            .dbus_state
            .connection
            .lock()
            .ok()
            .and_then(|c| c.clone())
        {
            let iface_ref = conn
                .object_server()
                .interface::<_, Self>("/com/handy/Transcription")
                .await;
            if let Ok(iface_ref) = iface_ref {
                if let Err(e) =
                    Self::recording_state_changed(iface_ref.signal_context(), is_recording).await
                {
                    error!("Failed to emit RecordingStateChanged signal: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn emit_error(&self, message: &str) -> fdo::Result<()> {
        if let Some(conn) = self
            .dbus_state
            .connection
            .lock()
            .ok()
            .and_then(|c| c.clone())
        {
            let iface_ref = conn
                .object_server()
                .interface::<_, Self>("/com/handy/Transcription")
                .await;
            if let Ok(iface_ref) = iface_ref {
                if let Err(e) = Self::error(iface_ref.signal_context(), message).await {
                    error!("Failed to emit Error signal: {}", e);
                }
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

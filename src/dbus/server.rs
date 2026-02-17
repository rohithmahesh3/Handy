//! D-Bus server for IBus integration
//!
//! This module provides a D-Bus interface that allows the handy-ibus engine
//! to control Handy's transcription functionality.

use crate::global_shortcuts::ptt_diagnostics_tuple;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{PostProcessProvider, Settings};
use crate::text_utils::convert_chinese_variant;
use crate::{audio_feedback::play_feedback_sound, audio_feedback::SoundType};
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::fdo;
use zbus::object_server::SignalContext;
use zbus::Connection;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";
const HANDY_INTERFACE: &str = "com.handy.Transcription";

const DEFAULT_BINDING_ID: &str = "ibus";

/// Shared state for the D-Bus server and handlers
pub struct HandyState {
    pub selected_language: Mutex<String>,
    pub recording_manager: Arc<AudioRecordingManager>,
    pub transcription_manager: Arc<TranscriptionManager>,
    pub is_recording: AtomicBool,
    partial_text: Mutex<String>,
    partial_sequence: AtomicU64,
    partial_session_id: AtomicU64,
    partial_cancel: Mutex<Option<Arc<AtomicBool>>>,
    session_counter: AtomicU64,
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
            partial_text: Mutex::new(String::new()),
            partial_sequence: AtomicU64::new(0),
            partial_session_id: AtomicU64::new(0),
            partial_cancel: Mutex::new(None),
            session_counter: AtomicU64::new(1),
        }
    }

    fn next_session_id(&self) -> u64 {
        self.session_counter.fetch_add(1, Ordering::SeqCst)
    }

    fn reset_partial_state(&self, session_id: u64) {
        *self.partial_text.lock().unwrap() = String::new();
        self.partial_sequence.store(0, Ordering::SeqCst);
        self.partial_session_id.store(session_id, Ordering::SeqCst);
    }

    fn clear_partial_state(&self) {
        *self.partial_text.lock().unwrap() = String::new();
        self.partial_sequence.store(0, Ordering::SeqCst);
    }

    fn latest_partial(&self) -> (u64, u64, String) {
        (
            self.partial_session_id.load(Ordering::SeqCst),
            self.partial_sequence.load(Ordering::SeqCst),
            self.partial_text.lock().unwrap().clone(),
        )
    }

    fn stop_partial_worker(&self) {
        if let Some(cancel) = self.partial_cancel.lock().unwrap().take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn start_partial_worker(self: &Arc<Self>, binding_id: String, session_id: u64) {
        self.stop_partial_worker();

        let settings = Settings::new();
        if !settings.live_partial_enabled() {
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        *self.partial_cancel.lock().unwrap() = Some(cancel.clone());

        let state = self.clone();
        let interval_ms = settings.live_partial_interval_ms().max(200);
        let min_chars_delta = settings.live_partial_min_chars_delta() as usize;
        let max_history_ms = settings.live_partial_max_history_ms().max(500);
        std::thread::spawn(move || {
            run_partial_worker(
                state,
                binding_id,
                session_id,
                cancel,
                interval_ms,
                min_chars_delta,
                max_history_ms,
            );
        });
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
    /// Start recording audio (compatibility method)
    async fn start_recording(&self) -> fdo::Result<()> {
        self.start_recording_internal(DEFAULT_BINDING_ID, 0).await
    }

    /// Stop recording and return transcribed text (compatibility method)
    async fn stop_recording(&self) -> fdo::Result<String> {
        self.stop_recording_internal(DEFAULT_BINDING_ID).await
    }

    /// Start a recording session and return session id
    async fn start_recording_session(&self) -> fdo::Result<u64> {
        let session_id = self.state.next_session_id();
        let binding_id = binding_id_for_session(session_id);
        self.start_recording_internal(&binding_id, session_id)
            .await?;
        Ok(session_id)
    }

    /// Stop a specific recording session and return final text
    async fn stop_recording_session(&self, session_id: u64) -> fdo::Result<String> {
        let binding_id = binding_id_for_session(session_id);
        self.stop_recording_internal(&binding_id).await
    }

    /// Cancel current recording without transcription
    async fn cancel_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: CancelRecording called");

        self.state.stop_partial_worker();
        self.state.clear_partial_state();
        self.state.recording_manager.cancel_recording();

        self.state.is_recording.store(false, Ordering::SeqCst);
        self.emit_recording_state_changed(false).await?;

        info!("D-Bus: Recording cancelled");
        Ok(())
    }

    /// Get current state: (is_recording, has_model_selected)
    async fn get_state(&self) -> fdo::Result<(bool, bool)> {
        let is_recording = self.state.is_recording.load(Ordering::SeqCst);
        let has_model = self.state.transcription_manager.has_model_selected();

        Ok((is_recording, has_model))
    }

    /// Get latest live partial: (session_id, sequence_id, text)
    async fn get_latest_partial(&self) -> fdo::Result<(u64, u64, String)> {
        Ok(self.state.latest_partial())
    }

    /// Get global push-to-talk diagnostics tuple
    async fn get_ptt_diagnostics(
        &self,
    ) -> fdo::Result<(bool, String, String, String, u64, bool, bool, u64, u64, u64)> {
        Ok(ptt_diagnostics_tuple())
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

    /// Signal emitted when a live partial update is available
    #[zbus(signal)]
    async fn partial_transcription_ready(
        ctxt: &SignalContext<'_>,
        session_id: u64,
        sequence_id: u64,
        text: &str,
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

    async fn start_recording_internal(&self, binding_id: &str, session_id: u64) -> fdo::Result<()> {
        debug!(
            "D-Bus: StartRecording called (binding='{}', session={})",
            binding_id, session_id
        );
        let start_time = Instant::now();

        if !self.state.transcription_manager.has_model_selected() {
            self.emit_error(
                "No model selected. Open Handy preferences to download and select a model.",
            )
            .await?;
            return Err(fdo::Error::Failed("No model selected".to_string()));
        }

        self.state.transcription_manager.initiate_model_load();

        let recording_started = self.state.recording_manager.try_start_recording(binding_id);
        if recording_started {
            let rm = self.state.recording_manager.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                rm.apply_mute();
            });
        }

        if recording_started {
            self.state.is_recording.store(true, Ordering::SeqCst);
            self.state.reset_partial_state(session_id);
            self.state
                .start_partial_worker(binding_id.to_string(), session_id);
            self.emit_recording_state_changed(true).await?;
            play_feedback_sound(&Settings::new(), SoundType::Start);
            info!("D-Bus: Recording started in {:?}", start_time.elapsed());
            Ok(())
        } else {
            self.emit_error("Failed to start recording").await?;
            Err(fdo::Error::Failed("Failed to start recording".to_string()))
        }
    }

    async fn stop_recording_internal(&self, binding_id: &str) -> fdo::Result<String> {
        debug!("D-Bus: StopRecording called for binding '{}'", binding_id);
        let stop_time = Instant::now();

        let was_recording = self.state.is_recording.swap(false, Ordering::SeqCst);
        if was_recording {
            self.emit_recording_state_changed(false).await?;
        }

        self.state.stop_partial_worker();
        play_feedback_sound(&Settings::new(), SoundType::Stop);
        self.state.recording_manager.remove_mute();

        if let Some(samples) = self.state.recording_manager.stop_recording(binding_id) {
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

                    self.state.clear_partial_state();
                    self.emit_transcription_ready(&output_text).await?;
                    Ok(output_text)
                }
                Err(err) => {
                    error!("D-Bus: Transcription error: {}", err);
                    self.state.clear_partial_state();
                    self.emit_error(&format!("Transcription failed: {}", err))
                        .await?;
                    Err(fdo::Error::Failed(format!("Transcription failed: {}", err)))
                }
            }
        } else {
            warn!("D-Bus: No samples retrieved from recording stop");
            self.state.clear_partial_state();
            Ok(String::new())
        }
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
                .interface::<_, Self>(HANDY_OBJECT_PATH)
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
                .interface::<_, Self>(HANDY_OBJECT_PATH)
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
                .interface::<_, Self>(HANDY_OBJECT_PATH)
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

fn binding_id_for_session(session_id: u64) -> String {
    format!("session-{}", session_id)
}

fn should_emit_partial(previous: &str, candidate: &str, min_chars_delta: usize) -> bool {
    if previous == candidate {
        return false;
    }
    if previous.is_empty() {
        return true;
    }

    let prev_len = previous.chars().count();
    let next_len = candidate.chars().count();
    let delta = prev_len.abs_diff(next_len);

    delta >= min_chars_delta || !candidate.starts_with(previous)
}

fn run_partial_worker(
    state: Arc<HandyState>,
    binding_id: String,
    session_id: u64,
    cancel: Arc<AtomicBool>,
    interval_ms: u32,
    min_chars_delta: usize,
    max_history_ms: u32,
) {
    let signal_conn = zbus::blocking::Connection::session().ok();

    let max_history_samples = ((max_history_ms as usize) * 16).max(1600);

    while !cancel.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(interval_ms as u64));

        if cancel.load(Ordering::SeqCst) || !state.is_recording.load(Ordering::SeqCst) {
            continue;
        }

        let Some(mut samples) = state.recording_manager.snapshot_recording(&binding_id) else {
            continue;
        };

        if samples.len() < 1600 {
            continue;
        }

        if samples.len() > max_history_samples {
            let start = samples.len() - max_history_samples;
            samples = samples[start..].to_vec();
        }

        let transcription = match state.transcription_manager.transcribe_partial(samples) {
            Ok(text) => text,
            Err(e) => {
                debug!("Live partial transcription failed: {}", e);
                continue;
            }
        };

        let lang = state.selected_language.lock().unwrap().clone();
        let converted = convert_chinese_variant(&transcription, &lang);
        let candidate = converted.trim().to_string();

        if candidate.is_empty() {
            continue;
        }

        let previous = state.partial_text.lock().unwrap().clone();
        if !should_emit_partial(&previous, &candidate, min_chars_delta) {
            continue;
        }

        *state.partial_text.lock().unwrap() = candidate.clone();
        state.partial_session_id.store(session_id, Ordering::SeqCst);
        let sequence_id = state.partial_sequence.fetch_add(1, Ordering::SeqCst) + 1;

        if let Some(conn) = signal_conn.as_ref() {
            let _ = conn.emit_signal(
                None::<&str>,
                HANDY_OBJECT_PATH,
                HANDY_INTERFACE,
                "PartialTranscriptionReady",
                &(session_id, sequence_id, candidate.as_str()),
            );
        }
    }
}

/// Start the D-Bus server
pub async fn start_dbus_server(state: Arc<HandyState>) -> Result<Arc<HandyDbusState>, String> {
    info!("Starting D-Bus server for IBus integration...");

    let dbus_state = Arc::new(HandyDbusState::new());

    let connection = Connection::session()
        .await
        .map_err(|e| format!("Failed to connect to session bus: {}", e))?;

    connection
        .request_name(HANDY_BUS_NAME)
        .await
        .map_err(|e| format!("Failed to request bus name: {}", e))?;

    let transcription = HandyTranscription::new(state, dbus_state.clone());

    connection
        .object_server()
        .at(HANDY_OBJECT_PATH, transcription)
        .await
        .map_err(|e| format!("Failed to register D-Bus object: {}", e))?;

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

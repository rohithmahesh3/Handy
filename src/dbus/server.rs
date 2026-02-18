//! D-Bus server for IBus integration
//!
//! This module provides a D-Bus interface that allows the handy-ibus engine
//! to control Handy's transcription functionality.

use crate::global_shortcuts::{
    ptt_diagnostics_tuple, ptt_diagnostics_verbose_json, ptt_recent_events,
};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{PostProcessProvider, Settings};
use crate::text_utils::convert_chinese_variant;
use crate::utils::logging::read_recent_logs;
use crate::{audio_feedback::play_feedback_sound, audio_feedback::SoundType};
use log::{debug, error, info};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::fdo;
use zbus::object_server::SignalContext;
use zbus::Connection;

const HANDY_BUS_NAME: &str = "com.handy.Transcription";
const HANDY_OBJECT_PATH: &str = "/com/handy/Transcription";

const DEFAULT_BINDING_ID: &str = "ibus";
const MAX_PENDING_COMMIT_QUEUE: usize = 32;

#[derive(Clone, Debug)]
struct PendingCommit {
    session_id: u64,
    target_engine_id: u64,
    text: String,
    created_ms: u64,
}

struct PendingCommitStore {
    inner: Mutex<VecDeque<PendingCommit>>,
    dropped_count: AtomicU64,
}

impl Default for PendingCommitStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(MAX_PENDING_COMMIT_QUEUE)),
            dropped_count: AtomicU64::new(0),
        }
    }
}

impl PendingCommitStore {
    fn store(&self, session_id: u64, target_engine_id: u64, text: String) {
        if let Ok(mut queue) = self.inner.lock() {
            if queue.len() >= MAX_PENDING_COMMIT_QUEUE {
                let _ = queue.pop_front();
                self.dropped_count.fetch_add(1, Ordering::SeqCst);
            }
            queue.push_back(PendingCommit {
                session_id,
                target_engine_id,
                text,
                created_ms: now_millis(),
            });
        }
    }

    fn take_for_engine(&self, target_engine_id: u64) -> (u64, String) {
        let Ok(mut queue) = self.inner.lock() else {
            return (0, String::new());
        };
        let Some(index) = queue
            .iter()
            .position(|entry| entry.target_engine_id == target_engine_id)
        else {
            return (0, String::new());
        };
        queue
            .remove(index)
            .map(|pending| (pending.session_id, pending.text))
            .unwrap_or_else(|| (0, String::new()))
    }

    fn stats_json(&self) -> String {
        let dropped_count = self.dropped_count.load(Ordering::SeqCst);
        if let Ok(queue) = self.inner.lock() {
            let now = now_millis();
            let oldest_age_ms = queue
                .front()
                .map(|entry| now.saturating_sub(entry.created_ms))
                .unwrap_or(0);
            let targets = queue
                .iter()
                .fold(HashMap::<u64, u64>::new(), |mut acc, item| {
                    *acc.entry(item.target_engine_id).or_insert(0) += 1;
                    acc
                });
            json!({
                "queue_len": queue.len(),
                "oldest_age_ms": oldest_age_ms,
                "dropped_count": dropped_count,
                "targets": targets,
            })
            .to_string()
        } else {
            json!({
                "queue_len": 0,
                "oldest_age_ms": 0,
                "dropped_count": dropped_count,
                "targets": {},
                "error": "lock_poisoned",
            })
            .to_string()
        }
    }
}

/// Shared state for the D-Bus server and handlers
pub struct HandyState {
    pub selected_language: Mutex<String>,
    pub recording_manager: Arc<AudioRecordingManager>,
    pub transcription_manager: Arc<TranscriptionManager>,
    pub is_recording: AtomicBool,
    session_counter: AtomicU64,
    /// Compatibility cache for legacy StopRecording callers.
    /// Push-to-talk commit handoff now uses `pending_commit`.
    last_transcription_cache: Mutex<Option<String>>,
    pending_commit: PendingCommitStore,
    focused_engine_id: AtomicU64,
    focused_engine_last_change_ms: AtomicU64,
    session_targets: Mutex<HashMap<u64, u64>>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
}

impl HandyState {
    pub fn new(
        recording_manager: Arc<AudioRecordingManager>,
        transcription_manager: Arc<TranscriptionManager>,
        selected_language: String,
        log_buffer: Arc<Mutex<VecDeque<String>>>,
    ) -> Self {
        Self {
            selected_language: Mutex::new(selected_language),
            recording_manager,
            transcription_manager,
            is_recording: AtomicBool::new(false),
            session_counter: AtomicU64::new(1),
            last_transcription_cache: Mutex::new(None),
            pending_commit: PendingCommitStore::default(),
            focused_engine_id: AtomicU64::new(0),
            focused_engine_last_change_ms: AtomicU64::new(now_millis()),
            session_targets: Mutex::new(HashMap::new()),
            log_buffer,
        }
    }

    fn next_session_id(&self) -> u64 {
        self.session_counter.fetch_add(1, Ordering::SeqCst)
    }

    fn recent_logs(&self, limit: usize) -> Vec<String> {
        read_recent_logs(&self.log_buffer, limit)
    }

    fn store_pending_commit(&self, session_id: u64, target_engine_id: u64, text: String) {
        self.pending_commit
            .store(session_id, target_engine_id, text);
    }

    fn take_pending_commit_for_engine(&self, target_engine_id: u64) -> (u64, String) {
        self.pending_commit.take_for_engine(target_engine_id)
    }

    fn pending_commit_stats_json(&self) -> String {
        self.pending_commit.stats_json()
    }

    fn set_focused_engine(&self, engine_id: u64, focused: bool) {
        let current = self.focused_engine_id.load(Ordering::SeqCst);
        let next = if focused {
            engine_id
        } else if current == engine_id {
            0
        } else {
            current
        };
        if next != current {
            self.focused_engine_id.store(next, Ordering::SeqCst);
            self.focused_engine_last_change_ms
                .store(now_millis(), Ordering::SeqCst);
        }
    }

    fn focused_engine_status(&self) -> (u64, u64) {
        (
            self.focused_engine_id.load(Ordering::SeqCst),
            self.focused_engine_last_change_ms.load(Ordering::SeqCst),
        )
    }

    fn register_session_target(&self, session_id: u64, target_engine_id: u64) {
        if let Ok(mut targets) = self.session_targets.lock() {
            targets.insert(session_id, target_engine_id);
        }
    }

    fn take_session_target(&self, session_id: u64) -> u64 {
        self.session_targets
            .lock()
            .ok()
            .and_then(|mut targets| targets.remove(&session_id))
            .unwrap_or(0)
    }

    fn clear_session_targets(&self) {
        if let Ok(mut targets) = self.session_targets.lock() {
            targets.clear();
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
    /// Start recording audio (compatibility method)
    async fn start_recording(&self) -> fdo::Result<()> {
        self.start_recording_internal(DEFAULT_BINDING_ID, None)
            .await
    }

    /// Stop recording and return transcribed text (compatibility method)
    async fn stop_recording(&self) -> fdo::Result<String> {
        self.stop_recording_internal(DEFAULT_BINDING_ID, None).await
    }

    /// Start a recording session and return session id
    async fn start_recording_session(&self) -> fdo::Result<u64> {
        let session_id = self.state.next_session_id();
        let (target_engine_id, _) = self.state.focused_engine_status();
        self.state
            .register_session_target(session_id, target_engine_id);
        let binding_id = binding_id_for_session(session_id);
        if let Err(e) = self
            .start_recording_internal(&binding_id, Some(session_id))
            .await
        {
            let _ = self.state.take_session_target(session_id);
            return Err(e);
        }
        Ok(session_id)
    }

    /// Start a recording session and bind commit routing to a focused engine id.
    async fn start_recording_session_for_target(&self, target_engine_id: u64) -> fdo::Result<u64> {
        if target_engine_id == 0 {
            return Err(fdo::Error::Failed(
                "Invalid target engine id 0 for session routing".to_string(),
            ));
        }
        let session_id = self.state.next_session_id();
        self.state
            .register_session_target(session_id, target_engine_id);
        let binding_id = binding_id_for_session(session_id);
        if let Err(e) = self
            .start_recording_internal(&binding_id, Some(session_id))
            .await
        {
            let _ = self.state.take_session_target(session_id);
            return Err(e);
        }
        Ok(session_id)
    }

    /// Stop a specific recording session and return final text
    async fn stop_recording_session(&self, session_id: u64) -> fdo::Result<String> {
        let binding_id = binding_id_for_session(session_id);
        self.stop_recording_internal(&binding_id, Some(session_id))
            .await
    }

    /// Cancel current recording without transcription
    async fn cancel_recording(&self) -> fdo::Result<()> {
        debug!("D-Bus: CancelRecording called");

        self.state.recording_manager.cancel_recording();
        self.state.clear_session_targets();

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

    /// Get global shortcut diagnostics tuple
    async fn get_ptt_diagnostics(
        &self,
    ) -> fdo::Result<(bool, String, String, String, u64, bool, bool, u64, u64, u64)> {
        Ok(ptt_diagnostics_tuple())
    }

    /// Get global shortcut diagnostics with verbose runtime fields.
    async fn get_ptt_diagnostics_verbose(&self) -> fdo::Result<String> {
        Ok(ptt_diagnostics_verbose_json())
    }

    /// Get recent global shortcut event lines.
    async fn get_ptt_recent_events(&self) -> fdo::Result<Vec<String>> {
        Ok(ptt_recent_events())
    }

    /// Atomically consume pending final text for a specific engine id.
    async fn take_pending_commit_for_engine(&self, engine_id: u64) -> fdo::Result<(u64, String)> {
        Ok(self.state.take_pending_commit_for_engine(engine_id))
    }

    /// Get aggregate pending commit queue stats as JSON.
    async fn get_pending_commit_stats(&self) -> fdo::Result<String> {
        Ok(self.state.pending_commit_stats_json())
    }

    /// Report focused engine transitions from IBus callbacks.
    async fn set_focused_engine(&self, engine_id: u64, focused: bool) -> fdo::Result<()> {
        self.state.set_focused_engine(engine_id, focused);
        Ok(())
    }

    /// Read currently focused engine id and last change timestamp.
    async fn get_focused_engine(&self) -> fdo::Result<(u64, u64)> {
        Ok(self.state.focused_engine_status())
    }

    /// Get recent daemon log lines
    async fn get_recent_logs(&self) -> fdo::Result<Vec<String>> {
        Ok(self.state.recent_logs(400))
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

    async fn start_recording_internal(
        &self,
        binding_id: &str,
        session_id: Option<u64>,
    ) -> fdo::Result<()> {
        let session_id_value = session_id.unwrap_or(0);
        debug!(
            "D-Bus: StartRecording called (binding='{}', session={})",
            binding_id, session_id_value
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

        match self.state.recording_manager.try_start_recording(binding_id) {
            Ok(()) => {
                let rm = self.state.recording_manager.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    rm.apply_mute();
                });

                self.state.is_recording.store(true, Ordering::SeqCst);
                self.emit_recording_state_changed(true).await?;
                play_feedback_sound(&Settings::new(), SoundType::Start);
                info!("D-Bus: Recording started in {:?}", start_time.elapsed());
                Ok(())
            }
            Err(err) => {
                let detail = err.detail();
                let message = format!("Failed to start recording ({}): {}", err.code(), detail);
                error!(
                    "D-Bus: StartRecording failed (binding='{}', session={}): {}",
                    binding_id, session_id_value, message
                );
                self.emit_error(&message).await?;
                Err(fdo::Error::Failed(message))
            }
        }
    }

    async fn stop_recording_internal(
        &self,
        binding_id: &str,
        session_id: Option<u64>,
    ) -> fdo::Result<String> {
        debug!("D-Bus: StopRecording called for binding '{}'", binding_id);
        let stop_time = Instant::now();
        let target_engine_id = session_id
            .map(|sid| self.state.take_session_target(sid))
            .unwrap_or(0);

        let was_recording = self.state.is_recording.swap(false, Ordering::SeqCst);
        if was_recording {
            self.emit_recording_state_changed(false).await?;
        }

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

                    // Cache transcription for engine process's subsequent stop_and_commit
                    if let Ok(mut cache) = self.state.last_transcription_cache.lock() {
                        *cache = Some(output_text.clone());
                    }
                    if let Some(session_id) = session_id {
                        if target_engine_id != 0 {
                            self.state.store_pending_commit(
                                session_id,
                                target_engine_id,
                                output_text.clone(),
                            );
                        }
                    }
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
            // Recording already stopped — return cached text from previous stop
            let cached = self
                .state
                .last_transcription_cache
                .lock()
                .ok()
                .and_then(|mut c| c.take())
                .unwrap_or_default();
            if !cached.is_empty() {
                debug!("D-Bus: Returning cached transcription text");
                if let Some(session_id) = session_id {
                    if target_engine_id != 0 {
                        self.state.store_pending_commit(
                            session_id,
                            target_engine_id,
                            cached.clone(),
                        );
                    }
                }
            }
            Ok(cached)
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

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::PendingCommitStore;
    use std::time::Duration;

    #[test]
    fn pending_commit_store_take_for_engine_consumes_target_only() {
        let store = PendingCommitStore::default();
        store.store(42, 11, "hello".to_string());
        store.store(43, 22, "world".to_string());

        let (session_id, text) = store.take_for_engine(11);
        assert_eq!(session_id, 42);
        assert_eq!(text, "hello");
        let (remaining_sid, remaining_text) = store.take_for_engine(22);
        assert_eq!(remaining_sid, 43);
        assert_eq!(remaining_text, "world");
    }

    #[test]
    fn pending_commit_store_stats_reports_oldest_age() {
        let store = PendingCommitStore::default();
        store.store(99, 17, "payload".to_string());
        std::thread::sleep(Duration::from_millis(2));
        let parsed: serde_json::Value =
            serde_json::from_str(&store.stats_json()).expect("valid stats json");
        let queue_len = parsed
            .get("queue_len")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let oldest_age_ms = parsed
            .get("oldest_age_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(queue_len, 1);
        assert!(oldest_age_ms > 0);
    }

    #[test]
    fn pending_commit_store_keeps_independent_queue_order() {
        let store = PendingCommitStore::default();
        store.store(10, 1, "first".to_string());
        store.store(11, 1, "second".to_string());
        store.store(12, 2, "third".to_string());

        let (sid1, text1) = store.take_for_engine(1);
        let (sid2, text2) = store.take_for_engine(1);
        let (sid3, text3) = store.take_for_engine(2);
        assert_eq!((sid1, text1), (10, "first".to_string()));
        assert_eq!((sid2, text2), (11, "second".to_string()));
        assert_eq!((sid3, text3), (12, "third".to_string()));
    }
}

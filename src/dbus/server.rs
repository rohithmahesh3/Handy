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
const LIVE_PREEDIT_POLL_MS: u64 = 600;
const LIVE_PREEDIT_MIN_NEW_SAMPLES: usize = 3200;
const LIVE_PREEDIT_MIN_TOTAL_SAMPLES: usize = 8000;
const LIVE_PREEDIT_MAX_WINDOW_SAMPLES: usize = 16000 * 8;
const LIVE_PREEDIT_SNAPSHOT_WARN_EVERY: u64 = 10;

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

#[derive(Clone, Debug)]
struct LivePreeditEntry {
    session_id: u64,
    revision: u64,
    visible: bool,
    text: String,
}

struct LivePreeditStore {
    inner: Mutex<HashMap<u64, LivePreeditEntry>>,
}

impl Default for LivePreeditStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl LivePreeditStore {
    fn set(&self, target_engine_id: u64, session_id: u64, revision: u64, text: String) {
        let Ok(mut entries) = self.inner.lock() else {
            return;
        };

        if let Some(existing) = entries.get(&target_engine_id) {
            if existing.session_id > session_id {
                return;
            }
            if existing.session_id == session_id && existing.revision >= revision {
                return;
            }
        }

        entries.insert(
            target_engine_id,
            LivePreeditEntry {
                session_id,
                revision,
                visible: true,
                text,
            },
        );
    }

    fn clear(&self, target_engine_id: u64, session_id: u64, revision: u64) {
        let Ok(mut entries) = self.inner.lock() else {
            return;
        };

        if let Some(existing) = entries.get(&target_engine_id) {
            if existing.session_id > session_id {
                return;
            }
            if existing.session_id == session_id && existing.revision >= revision {
                return;
            }
        }

        entries.insert(
            target_engine_id,
            LivePreeditEntry {
                session_id,
                revision,
                visible: false,
                text: String::new(),
            },
        );
    }

    fn get_for_engine(&self, target_engine_id: u64) -> (u64, u64, bool, String) {
        let Ok(entries) = self.inner.lock() else {
            return (0, 0, false, String::new());
        };

        entries
            .get(&target_engine_id)
            .map(|entry| {
                (
                    entry.session_id,
                    entry.revision,
                    entry.visible,
                    entry.text.clone(),
                )
            })
            .unwrap_or((0, 0, false, String::new()))
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
    live_preedit: LivePreeditStore,
    live_preedit_revision: AtomicU64,
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
            live_preedit: LivePreeditStore::default(),
            live_preedit_revision: AtomicU64::new(1),
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

    fn next_live_preedit_revision(&self) -> u64 {
        self.live_preedit_revision.fetch_add(1, Ordering::SeqCst)
    }

    fn set_live_preedit(
        &self,
        target_engine_id: u64,
        session_id: u64,
        revision: u64,
        text: String,
    ) {
        if target_engine_id == 0 || session_id == 0 {
            return;
        }
        self.live_preedit
            .set(target_engine_id, session_id, revision, text);
    }

    fn clear_live_preedit(&self, target_engine_id: u64, session_id: u64, revision: u64) {
        if target_engine_id == 0 || session_id == 0 {
            return;
        }
        self.live_preedit
            .clear(target_engine_id, session_id, revision);
    }

    fn get_live_preedit_for_engine(&self, target_engine_id: u64) -> (u64, u64, bool, String) {
        self.live_preedit.get_for_engine(target_engine_id)
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

    fn session_target(&self, session_id: u64) -> Option<u64> {
        self.session_targets
            .lock()
            .ok()
            .and_then(|targets| targets.get(&session_id).copied())
    }

    fn take_session_target(&self, session_id: u64) -> u64 {
        self.session_targets
            .lock()
            .ok()
            .and_then(|mut targets| targets.remove(&session_id))
            .unwrap_or(0)
    }

    fn session_targets_snapshot(&self) -> Vec<(u64, u64)> {
        self.session_targets
            .lock()
            .map(|targets| targets.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default()
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

        for (session_id, target_engine_id) in self.state.session_targets_snapshot() {
            if target_engine_id != 0 {
                let revision = self.state.next_live_preedit_revision();
                self.state
                    .clear_live_preedit(target_engine_id, session_id, revision);
            }
        }

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

    /// Read latest live preedit payload for the engine.
    async fn get_live_preedit_for_engine(
        &self,
        engine_id: u64,
    ) -> fdo::Result<(u64, u64, bool, String)> {
        Ok(self.state.get_live_preedit_for_engine(engine_id))
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

                if Settings::new().experimental_enabled() {
                    if let Some(session_id) = session_id {
                        if let Some(target_engine_id) = self.state.session_target(session_id) {
                            if target_engine_id != 0 {
                                let revision = self.state.next_live_preedit_revision();
                                self.state.clear_live_preedit(
                                    target_engine_id,
                                    session_id,
                                    revision,
                                );
                                spawn_live_preedit_worker(
                                    self.state.clone(),
                                    binding_id.to_string(),
                                    session_id,
                                    target_engine_id,
                                );
                            }
                        }
                    }
                }

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
        let live_session_id = session_id.unwrap_or(0);
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
                    if live_session_id != 0 && target_engine_id != 0 {
                        let revision = self.state.next_live_preedit_revision();
                        self.state
                            .clear_live_preedit(target_engine_id, live_session_id, revision);
                    }
                    self.emit_transcription_ready(&output_text).await?;
                    Ok(output_text)
                }
                Err(err) => {
                    if live_session_id != 0 && target_engine_id != 0 {
                        let revision = self.state.next_live_preedit_revision();
                        self.state
                            .clear_live_preedit(target_engine_id, live_session_id, revision);
                    }
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
            if live_session_id != 0 && target_engine_id != 0 {
                let revision = self.state.next_live_preedit_revision();
                self.state
                    .clear_live_preedit(target_engine_id, live_session_id, revision);
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

fn spawn_live_preedit_worker(
    state: Arc<HandyState>,
    binding_id: String,
    session_id: u64,
    target_engine_id: u64,
) {
    std::thread::spawn(move || {
        let mut last_snapshot_len: usize = 0;
        let mut snapshot_failure_streak: u64 = 0;
        let mut published_text = String::new();
        let mut last_window_text = String::new();
        let mut accumulated_text = String::new();

        loop {
            if state.session_target(session_id) != Some(target_engine_id) {
                break;
            }
            if !state.is_recording.load(Ordering::SeqCst) {
                break;
            }

            std::thread::sleep(Duration::from_millis(LIVE_PREEDIT_POLL_MS));

            let Some(samples) = state.recording_manager.snapshot_recording(&binding_id) else {
                // Snapshot failures can be transient under load; keep the last preview visible
                // and continue retrying while this session is still active.
                if state.session_target(session_id) != Some(target_engine_id)
                    || !state.is_recording.load(Ordering::SeqCst)
                {
                    break;
                }
                snapshot_failure_streak = snapshot_failure_streak.saturating_add(1);
                if snapshot_failure_streak == 1
                    || snapshot_failure_streak.is_multiple_of(LIVE_PREEDIT_SNAPSHOT_WARN_EVERY)
                {
                    debug!(
                        "Live preedit snapshot unavailable for session {} (streak={}); retaining current preview",
                        session_id, snapshot_failure_streak
                    );
                }
                continue;
            };

            if snapshot_failure_streak > 0 {
                debug!(
                    "Live preedit snapshot recovered for session {} after {} transient misses",
                    session_id, snapshot_failure_streak
                );
                snapshot_failure_streak = 0;
            }

            if samples.len() < LIVE_PREEDIT_MIN_TOTAL_SAMPLES {
                continue;
            }

            if last_snapshot_len > 0
                && samples.len().saturating_sub(last_snapshot_len) < LIVE_PREEDIT_MIN_NEW_SAMPLES
            {
                continue;
            }
            last_snapshot_len = samples.len();

            let live_window = if samples.len() > LIVE_PREEDIT_MAX_WINDOW_SAMPLES {
                samples[samples.len() - LIVE_PREEDIT_MAX_WINDOW_SAMPLES..].to_vec()
            } else {
                samples
            };

            let transcription = match state.transcription_manager.transcribe_for_live(live_window) {
                Ok(text) => text,
                Err(err) => {
                    debug!(
                        "Live preedit transcription failed for session {}: {}",
                        session_id, err
                    );
                    continue;
                }
            };

            let lang = state.selected_language.lock().unwrap().clone();
            let live_text = convert_chinese_variant(&transcription, &lang)
                .trim()
                .to_string();

            if state.session_target(session_id) != Some(target_engine_id) {
                break;
            }

            if live_text.is_empty() {
                continue;
            }

            if accumulated_text.is_empty() {
                accumulated_text = live_text.clone();
            } else {
                accumulated_text =
                    merge_live_transcript(&accumulated_text, &last_window_text, &live_text);
            }
            last_window_text = live_text;

            if accumulated_text != published_text {
                let revision = state.next_live_preedit_revision();
                state.set_live_preedit(
                    target_engine_id,
                    session_id,
                    revision,
                    accumulated_text.clone(),
                );
                published_text = accumulated_text.clone();
            }
        }

        if !published_text.is_empty() {
            let revision = state.next_live_preedit_revision();
            state.clear_live_preedit(target_engine_id, session_id, revision);
        }
    });
}

fn merge_live_transcript(accumulated: &str, prev_window: &str, next_window: &str) -> String {
    if accumulated.is_empty() || prev_window.is_empty() {
        return next_window.to_string();
    }
    if next_window.is_empty() || next_window == prev_window {
        return accumulated.to_string();
    }
    if let Some(base) = accumulated.strip_suffix(prev_window) {
        if next_window.starts_with(prev_window) {
            return format!("{}{}", base, next_window);
        }

        let lcp = common_prefix_chars(prev_window, next_window);
        let prev_len = prev_window.chars().count();
        let next_len = next_window.chars().count();
        if lcp >= 8 || (lcp * 2 >= prev_len.min(next_len) && lcp >= 3) {
            return format!("{}{}", base, next_window);
        }

        let overlap = longest_suffix_prefix_chars(prev_window, next_window);
        if overlap > 0 {
            let overlap_bytes = byte_index_at_char(next_window, overlap);
            return format!("{}{}", accumulated, &next_window[overlap_bytes..]);
        }
    }

    if accumulated.ends_with(next_window) {
        return accumulated.to_string();
    }

    format!("{}{}", accumulated, next_window)
}

fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(a, b)| a == b)
        .count()
}

fn longest_suffix_prefix_chars(left: &str, right: &str) -> usize {
    let left_bounds = char_boundaries(left);
    let right_bounds = char_boundaries(right);
    let max = left_bounds
        .len()
        .saturating_sub(1)
        .min(right_bounds.len().saturating_sub(1));
    for overlap_chars in (1..=max).rev() {
        let left_start = left_bounds[left_bounds.len() - 1 - overlap_chars];
        let right_end = right_bounds[overlap_chars];
        if left[left_start..] == right[..right_end] {
            return overlap_chars;
        }
    }
    0
}

fn byte_index_at_char(text: &str, char_idx: usize) -> usize {
    char_boundaries(text)
        .get(char_idx)
        .copied()
        .unwrap_or(text.len())
}

fn char_boundaries(text: &str) -> Vec<usize> {
    let mut bounds = text.char_indices().map(|(idx, _)| idx).collect::<Vec<_>>();
    bounds.push(text.len());
    bounds
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
    use super::{LivePreeditStore, PendingCommitStore};
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

    #[test]
    fn live_preedit_store_tracks_latest_per_engine() {
        let store = LivePreeditStore::default();
        store.set(11, 42, 1, "alpha".to_string());
        store.set(11, 42, 2, "bravo".to_string());

        let (session_id, revision, visible, text) = store.get_for_engine(11);
        assert_eq!(session_id, 42);
        assert_eq!(revision, 2);
        assert!(visible);
        assert_eq!(text, "bravo");
    }

    #[test]
    fn live_preedit_store_keeps_engine_isolation() {
        let store = LivePreeditStore::default();
        store.set(11, 21, 7, "left".to_string());
        store.set(22, 22, 9, "right".to_string());

        let left = store.get_for_engine(11);
        let right = store.get_for_engine(22);
        assert_eq!(left.0, 21);
        assert_eq!(left.1, 7);
        assert_eq!(left.3, "left");
        assert_eq!(right.0, 22);
        assert_eq!(right.1, 9);
        assert_eq!(right.3, "right");
    }

    #[test]
    fn live_preedit_store_clear_hides_entry() {
        let store = LivePreeditStore::default();
        store.set(44, 101, 3, "hello".to_string());
        store.clear(44, 101, 4);

        let (session_id, revision, visible, text) = store.get_for_engine(44);
        assert_eq!(session_id, 101);
        assert_eq!(revision, 4);
        assert!(!visible);
        assert!(text.is_empty());
    }

    #[test]
    fn merge_live_transcript_appends_shifted_tail_without_losing_prefix() {
        let accumulated = "hello world";
        let prev = "hello world";
        let next = "world again";
        let merged = super::merge_live_transcript(accumulated, prev, next);
        assert_eq!(merged, "hello world again");
    }

    #[test]
    fn merge_live_transcript_replaces_tail_on_correction() {
        let accumulated = "hello wurld";
        let prev = "hello wurld";
        let next = "hello world";
        let merged = super::merge_live_transcript(accumulated, prev, next);
        assert_eq!(merged, "hello world");
    }
}

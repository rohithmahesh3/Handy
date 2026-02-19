use crate::audio_toolkit::{apply_custom_words, filter_transcription_output};
use crate::managers::model::{EngineType, ModelInfo, ModelManager};
use crate::settings::{InferenceDevicePolicy, ModelUnloadTimeout, Settings};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use transcribe_rs::{
    engines::{
        moonshine::{ModelVariant, MoonshineEngine, MoonshineModelParams},
        parakeet::{ParakeetEngine, ParakeetModelParams},
        sense_voice::{SenseVoiceEngine, SenseVoiceModelParams},
        whisper::{WhisperEngine, WhisperInferenceParams},
    },
    onnx_execution::{
        detect_onnx_execution_capabilities, OnnxExecutionCapabilities, OnnxExecutionDevice,
        OnnxExecutionParams,
    },
    TranscriptionEngine,
};

enum LoadedEngine {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
    Moonshine(MoonshineEngine),
    SenseVoice(SenseVoiceEngine),
}

const LOAD_RETRY_COOLDOWN_MS: u64 = 3000;

struct ModelLoadFailure {
    model_id: String,
    message: String,
    at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct InferenceRuntimeStatus {
    pub policy: InferenceDevicePolicy,
    pub requested_gpu_device_id: i32,
    pub requested_device: OnnxExecutionDevice,
    pub effective_device: OnnxExecutionDevice,
    pub fallback_used: bool,
    pub cuda_available: bool,
    pub reason: String,
    pub last_error: Option<String>,
    pub model_id: Option<String>,
    pub updated_ms: u64,
}

impl InferenceRuntimeStatus {
    fn pending(config: &TranscriptionConfig, message: &str) -> Self {
        Self {
            policy: config.inference_device_policy,
            requested_gpu_device_id: config.inference_gpu_device_id.max(0),
            requested_device: TranscriptionManager::requested_device_from_policy(
                config.inference_device_policy,
            ),
            effective_device: OnnxExecutionDevice::Cpu,
            fallback_used: false,
            cuda_available: detect_onnx_execution_capabilities().cuda_available,
            reason: message.to_string(),
            last_error: None,
            model_id: None,
            updated_ms: TranscriptionManager::now_ms(),
        }
    }
}

#[derive(Clone)]
pub struct TranscriptionConfig {
    pub model_unload_timeout: ModelUnloadTimeout,
    pub selected_language: String,
    pub translate_to_english: bool,
    pub custom_words: Vec<String>,
    pub word_correction_threshold: f64,
    pub inference_device_policy: InferenceDevicePolicy,
    pub inference_gpu_device_id: i32,
}

impl TranscriptionConfig {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            model_unload_timeout: settings.model_unload_timeout(),
            selected_language: settings.selected_language(),
            translate_to_english: settings.translate_to_english(),
            custom_words: settings.custom_words(),
            word_correction_threshold: settings.word_correction_threshold(),
            inference_device_policy: settings.inference_device_policy(),
            inference_gpu_device_id: settings.inference_gpu_device_id(),
        }
    }
}

struct SharedState {
    engine: Mutex<Option<LoadedEngine>>,
    config: Mutex<TranscriptionConfig>,
    current_model_id: Mutex<Option<String>>,
    inference_runtime_status: Mutex<InferenceRuntimeStatus>,
    last_activity: AtomicU64,
    is_loading: Mutex<bool>,
    loading_condvar: Condvar,
    last_load_failure: Mutex<Option<ModelLoadFailure>>,
}

pub struct TranscriptionManager {
    shared: Arc<SharedState>,
    model_manager: Arc<ModelManager>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl TranscriptionManager {
    pub fn new(model_manager: Arc<ModelManager>) -> Result<Self> {
        let settings = Settings::new();
        let config = TranscriptionConfig::from_settings(&settings);
        let _unload_timeout = config.model_unload_timeout;
        let runtime_status =
            InferenceRuntimeStatus::pending(&config, "Runtime not initialized (model not loaded)");

        let shared = Arc::new(SharedState {
            engine: Mutex::new(None),
            config: Mutex::new(config),
            current_model_id: Mutex::new(None),
            inference_runtime_status: Mutex::new(runtime_status),
            last_activity: AtomicU64::new(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            ),
            is_loading: Mutex::new(false),
            loading_condvar: Condvar::new(),
            last_load_failure: Mutex::new(None),
        });

        let shutdown_signal = Arc::new(AtomicBool::new(false));

        {
            let shared_clone = shared.clone();
            let shutdown_signal_clone = shutdown_signal.clone();
            let handle = thread::spawn(move || {
                while !shutdown_signal_clone.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10));

                    if shutdown_signal_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let config = shared_clone.config.lock().unwrap();
                    let timeout = config.model_unload_timeout;
                    drop(config);

                    let timeout_seconds = timeout.to_seconds();

                    if let Some(limit_seconds) = timeout_seconds {
                        if limit_seconds == 0 {
                            continue; // Handled by maybe_unload_immediately()
                        }

                        let last = shared_clone.last_activity.load(Ordering::Relaxed);
                        let now_ms = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;

                        if now_ms.saturating_sub(last) > limit_seconds * 1000 {
                            let mut engine = shared_clone.engine.lock().unwrap();
                            if engine.is_some() {
                                debug!("Unloading model due to inactivity");
                                *engine = None;
                                drop(engine);
                                *shared_clone.current_model_id.lock().unwrap() = None;
                                if let Ok(mut runtime_status) =
                                    shared_clone.inference_runtime_status.lock()
                                {
                                    runtime_status.model_id = None;
                                    runtime_status.reason =
                                        "Model unloaded due to inactivity".to_string();
                                    runtime_status.updated_ms = TranscriptionManager::now_ms();
                                }
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down");
            });

            let manager = Self {
                shared,
                model_manager,
                shutdown_signal,
                watcher_handle: Mutex::new(Some(handle)),
            };

            Ok(manager)
        }
    }

    pub fn is_model_loaded(&self) -> bool {
        let engine = self.shared.engine.lock().unwrap();
        engine.is_some()
    }

    /// Returns true if a model is selected in settings AND downloaded to disk.
    /// This is distinct from `is_model_loaded()` which checks if the engine is
    /// currently loaded in memory (it may have been unloaded by the idle timeout).
    pub fn has_model_selected(&self) -> bool {
        let selected = self.model_manager.get_current_model();
        if selected.is_empty() {
            return false;
        }
        self.model_manager
            .get_model_info(&selected)
            .map(|m| m.is_downloaded)
            .unwrap_or(false)
    }

    pub fn unload_model(&self) -> Result<()> {
        debug!("Unloading model");

        {
            let mut engine = self.shared.engine.lock().unwrap();
            if let Some(ref mut loaded_engine) = *engine {
                match loaded_engine {
                    LoadedEngine::Whisper(ref mut e) => e.unload_model(),
                    LoadedEngine::Parakeet(ref mut e) => e.unload_model(),
                    LoadedEngine::Moonshine(ref mut e) => e.unload_model(),
                    LoadedEngine::SenseVoice(ref mut e) => e.unload_model(),
                }
            }
            *engine = None;
        }
        {
            let mut current_model = self.shared.current_model_id.lock().unwrap();
            *current_model = None;
        }
        {
            let mut runtime_status = self.shared.inference_runtime_status.lock().unwrap();
            runtime_status.model_id = None;
            runtime_status.reason = "Model unloaded".to_string();
            runtime_status.updated_ms = Self::now_ms();
        }

        debug!("Model unloaded");
        Ok(())
    }

    pub fn maybe_unload_immediately(&self, context: &str) {
        let config = self.shared.config.lock().unwrap();
        if config.model_unload_timeout == ModelUnloadTimeout::Immediately && self.is_model_loaded()
        {
            info!("Immediately unloading model after {}", context);
            drop(config);
            let _ = self.unload_model();
        }
    }

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        debug!("Loading model: {}", model_id);

        let model_info = self
            .model_manager
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            return Err(anyhow::anyhow!("Model not downloaded"));
        }

        let model_path = self
            .model_manager
            .get_model_path(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model path not found"))?;
        let config = { self.shared.config.lock().unwrap().clone() };
        let (loaded_engine, runtime_status) =
            match Self::load_engine_for_config(&model_info, &model_path, &config) {
                Ok(result) => result,
                Err(e) => {
                    let status =
                        Self::build_failure_runtime_status(&config, model_id, e.to_string());
                    self.set_inference_runtime_status(status);
                    return Err(anyhow::anyhow!("Failed to load {} model: {}", model_id, e));
                }
            };

        {
            let mut engine = self.shared.engine.lock().unwrap();
            *engine = Some(loaded_engine);
        }
        {
            let mut current_model = self.shared.current_model_id.lock().unwrap();
            *current_model = Some(model_id.to_string());
        }
        self.set_inference_runtime_status(runtime_status);

        info!("Model {} loaded successfully", model_id);
        Ok(())
    }

    pub fn initiate_model_load(&self) {
        if self.is_model_loaded() {
            return;
        }

        let selected_model = self.model_manager.get_current_model();
        if selected_model.is_empty() {
            warn!("No model selected");
            return;
        }

        if self.should_throttle_load_attempt(&selected_model) {
            debug!(
                "Skipping immediate retry for model {} due to recent load failure",
                selected_model
            );
            return;
        }

        let mut is_loading = self.shared.is_loading.lock().unwrap();
        if *is_loading {
            return;
        }
        *is_loading = true;
        {
            let config = self.shared.config.lock().unwrap().clone();
            let mut status = InferenceRuntimeStatus::pending(&config, "Initializing model runtime");
            status.model_id = Some(selected_model.clone());
            self.set_inference_runtime_status(status);
        }
        let shared = self.shared.clone();
        let model_manager = self.model_manager.clone();
        drop(is_loading);

        thread::spawn(move || {
            let model_info = model_manager.get_model_info(&selected_model);
            if model_info.is_none() || !model_info.as_ref().unwrap().is_downloaded {
                let message = format!("Model not found or not downloaded: {}", selected_model);
                error!("{}", message);
                Self::set_load_failure(&shared, &selected_model, message);
                let mut is_loading = shared.is_loading.lock().unwrap();
                *is_loading = false;
                shared.loading_condvar.notify_all();
                return;
            }

            let model_path = model_manager.get_model_path(&selected_model);
            if model_path.is_none() {
                let message = format!("Model path not found: {}", selected_model);
                error!("{}", message);
                Self::set_load_failure(&shared, &selected_model, message);
                let mut is_loading = shared.is_loading.lock().unwrap();
                *is_loading = false;
                shared.loading_condvar.notify_all();
                return;
            }

            let model_path = model_path.unwrap();
            let model_info = model_info.unwrap();
            let config = { shared.config.lock().unwrap().clone() };
            match Self::load_engine_for_config(&model_info, &model_path, &config) {
                Ok((loaded_engine, runtime_status)) => {
                    *shared.engine.lock().unwrap() = Some(loaded_engine);
                    *shared.current_model_id.lock().unwrap() = Some(selected_model.clone());
                    Self::clear_load_failure(&shared, &selected_model);
                    Self::set_inference_runtime_status_shared(&shared, runtime_status);
                    info!("Model {} loaded successfully", selected_model);
                }
                Err(e) => {
                    error!("{}", e);
                    let status =
                        Self::build_failure_runtime_status(&config, &selected_model, e.to_string());
                    Self::set_inference_runtime_status_shared(&shared, status);
                    Self::set_load_failure(&shared, &selected_model, e.to_string());
                }
            }

            let mut is_loading = shared.is_loading.lock().unwrap();
            *is_loading = false;
            shared.loading_condvar.notify_all();
        });
    }

    fn transcribe_internal(
        &self,
        samples: Vec<f32>,
        allow_immediate_unload: bool,
    ) -> Result<String> {
        self.update_activity();

        let model_id = {
            let current = self.shared.current_model_id.lock().unwrap();
            current.clone()
        };

        if model_id.is_none() || !self.is_model_loaded() {
            self.initiate_model_load();

            let mut is_loading = self.shared.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.shared.loading_condvar.wait(is_loading).unwrap();
            }
        }

        let mut engine = self.shared.engine.lock().unwrap();
        if engine.is_none() {
            drop(engine);
            if let Some(message) = self.selected_model_failure_message() {
                return Err(anyhow::anyhow!("No engine loaded: {}", message));
            }
            return Err(anyhow::anyhow!("No engine loaded"));
        }
        let loaded_engine = engine.as_mut().unwrap();

        let (language, translate, custom_words, threshold) = {
            let config = self.shared.config.lock().unwrap();
            (
                config.selected_language.clone(),
                config.translate_to_english,
                config.custom_words.clone(),
                config.word_correction_threshold,
            )
        };

        let result = match loaded_engine {
            LoadedEngine::Whisper(e) => {
                let mut params = WhisperInferenceParams::default();
                if language != "auto" {
                    params.language = Some(language.clone());
                }
                params.translate = translate;
                e.transcribe_samples(samples.clone(), Some(params))
                    .map_err(|e| anyhow::anyhow!("Whisper transcription failed: {}", e))
            }
            LoadedEngine::Parakeet(e) => e
                .transcribe_samples(samples.clone(), None)
                .map_err(|e| anyhow::anyhow!("Parakeet transcription failed: {}", e)),
            LoadedEngine::Moonshine(e) => e
                .transcribe_samples(samples.clone(), None)
                .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {}", e)),
            LoadedEngine::SenseVoice(e) => e
                .transcribe_samples(samples, None)
                .map_err(|e| anyhow::anyhow!("SenseVoice transcription failed: {}", e)),
        };

        drop(engine);

        let transcription_result = result?;
        let mut text = transcription_result.text;

        if !custom_words.is_empty() {
            text = apply_custom_words(&text, &custom_words, threshold);
        }

        text = filter_transcription_output(&text);

        if allow_immediate_unload {
            self.maybe_unload_immediately("transcription");
        }

        Ok(text)
    }

    pub fn transcribe(&self, samples: Vec<f32>) -> Result<String> {
        self.transcribe_internal(samples, true)
    }

    pub fn transcribe_for_live(&self, samples: Vec<f32>) -> Result<String> {
        self.transcribe_internal(samples, false)
    }

    pub fn refresh_config_from_settings(&self, settings: &Settings) {
        let updated = TranscriptionConfig::from_settings(settings);
        {
            let mut config = self.shared.config.lock().unwrap();
            *config = updated.clone();
        }
        let mut runtime_status = self.shared.inference_runtime_status.lock().unwrap();
        runtime_status.policy = updated.inference_device_policy;
        runtime_status.requested_gpu_device_id = updated.inference_gpu_device_id.max(0);
        runtime_status.requested_device =
            Self::requested_device_from_policy(updated.inference_device_policy);
        runtime_status.cuda_available = detect_onnx_execution_capabilities().cuda_available;
        runtime_status.reason =
            "Configuration updated; reload model to apply inference changes".to_string();
        runtime_status.updated_ms = Self::now_ms();
    }

    pub fn get_model_load_status(&self) -> (bool, bool, Option<String>) {
        let is_loading = *self.shared.is_loading.lock().unwrap();
        let is_loaded = self.is_model_loaded();
        let current_model = self.shared.current_model_id.lock().unwrap().clone();
        (is_loading, is_loaded, current_model)
    }

    pub fn inference_runtime_status_json(&self) -> String {
        let status = self.shared.inference_runtime_status.lock().unwrap().clone();
        json!({
            "policy": status.policy.as_str(),
            "requested_device": status.requested_device.as_str(),
            "requested_gpu_device_id": status.requested_gpu_device_id,
            "effective_device": status.effective_device.as_str(),
            "fallback_used": status.fallback_used,
            "cuda_available": status.cuda_available,
            "reason": status.reason,
            "last_error": status.last_error,
            "model_id": status.model_id,
            "updated_ms": status.updated_ms,
        })
        .to_string()
    }

    fn update_activity(&self) {
        let now = Self::now_ms();
        self.shared.last_activity.store(now, Ordering::Relaxed);
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn set_load_failure(shared: &Arc<SharedState>, model_id: &str, message: String) {
        let mut failure = shared.last_load_failure.lock().unwrap();
        *failure = Some(ModelLoadFailure {
            model_id: model_id.to_string(),
            message,
            at_ms: Self::now_ms(),
        });
    }

    fn clear_load_failure(shared: &Arc<SharedState>, model_id: &str) {
        let mut failure = shared.last_load_failure.lock().unwrap();
        if failure
            .as_ref()
            .map(|f| f.model_id == model_id)
            .unwrap_or(false)
        {
            *failure = None;
        }
    }

    fn should_throttle_load_attempt(&self, model_id: &str) -> bool {
        let failure = self.shared.last_load_failure.lock().unwrap();
        if let Some(failure) = failure.as_ref() {
            if failure.model_id != model_id {
                return false;
            }
            let elapsed = Self::now_ms().saturating_sub(failure.at_ms);
            return elapsed < LOAD_RETRY_COOLDOWN_MS;
        }
        false
    }

    fn selected_model_failure_message(&self) -> Option<String> {
        let selected_model = self.model_manager.get_current_model();
        if selected_model.is_empty() {
            return None;
        }

        let failure = self.shared.last_load_failure.lock().unwrap();
        failure.as_ref().and_then(|failure| {
            if failure.model_id == selected_model {
                Some(format!(
                    "failed to load model {}: {}",
                    selected_model, failure.message
                ))
            } else {
                None
            }
        })
    }

    fn requested_device_from_policy(policy: InferenceDevicePolicy) -> OnnxExecutionDevice {
        match policy {
            InferenceDevicePolicy::Auto => OnnxExecutionDevice::Auto,
            InferenceDevicePolicy::Cpu => OnnxExecutionDevice::Cpu,
            InferenceDevicePolicy::Gpu => OnnxExecutionDevice::Gpu,
        }
    }

    fn onnx_execution_params_for_device(
        config: &TranscriptionConfig,
        device: OnnxExecutionDevice,
        allow_cpu_fallback: bool,
    ) -> OnnxExecutionParams {
        OnnxExecutionParams {
            device,
            gpu_device_id: config.inference_gpu_device_id.max(0),
            allow_cpu_fallback,
        }
    }

    fn is_onnx_engine(engine_type: &EngineType) -> bool {
        !matches!(engine_type, EngineType::Whisper)
    }

    fn load_engine_with_params(
        model_info: &ModelInfo,
        model_path: &Path,
        onnx_execution: &OnnxExecutionParams,
    ) -> Result<LoadedEngine> {
        let loaded_engine = match model_info.engine_type {
            EngineType::Whisper => {
                if matches!(onnx_execution.device, OnnxExecutionDevice::Gpu) {
                    info!(
                        "Whisper engine ignores ONNX GPU settings; using whisper backend defaults"
                    );
                }
                let mut engine = WhisperEngine::new();
                engine
                    .load_model(model_path)
                    .map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {}", e))?;
                LoadedEngine::Whisper(engine)
            }
            EngineType::Parakeet => {
                let mut engine = ParakeetEngine::new();
                let mut params = ParakeetModelParams::int8();
                params.execution = onnx_execution.clone();
                engine
                    .load_model_with_params(model_path, params)
                    .map_err(|e| anyhow::anyhow!("Failed to load Parakeet model: {}", e))?;
                LoadedEngine::Parakeet(engine)
            }
            EngineType::Moonshine => {
                let mut engine = MoonshineEngine::new();
                let mut params = MoonshineModelParams::variant(ModelVariant::Base);
                params.execution = onnx_execution.clone();
                engine
                    .load_model_with_params(model_path, params)
                    .map_err(|e| anyhow::anyhow!("Failed to load Moonshine model: {}", e))?;
                LoadedEngine::Moonshine(engine)
            }
            EngineType::SenseVoice => {
                let mut engine = SenseVoiceEngine::new();
                let mut params = SenseVoiceModelParams::int8();
                params.execution = onnx_execution.clone();
                engine
                    .load_model_with_params(model_path, params)
                    .map_err(|e| anyhow::anyhow!("Failed to load SenseVoice model: {}", e))?;
                LoadedEngine::SenseVoice(engine)
            }
        };
        Ok(loaded_engine)
    }

    fn build_success_runtime_status(
        config: &TranscriptionConfig,
        model_id: &str,
        capabilities: OnnxExecutionCapabilities,
        effective_device: OnnxExecutionDevice,
        fallback_used: bool,
        reason: String,
        last_error: Option<String>,
    ) -> InferenceRuntimeStatus {
        InferenceRuntimeStatus {
            policy: config.inference_device_policy,
            requested_gpu_device_id: config.inference_gpu_device_id.max(0),
            requested_device: Self::requested_device_from_policy(config.inference_device_policy),
            effective_device,
            fallback_used,
            cuda_available: capabilities.cuda_available,
            reason,
            last_error,
            model_id: Some(model_id.to_string()),
            updated_ms: Self::now_ms(),
        }
    }

    fn build_failure_runtime_status(
        config: &TranscriptionConfig,
        model_id: &str,
        message: String,
    ) -> InferenceRuntimeStatus {
        let caps = detect_onnx_execution_capabilities();
        InferenceRuntimeStatus {
            policy: config.inference_device_policy,
            requested_gpu_device_id: config.inference_gpu_device_id.max(0),
            requested_device: Self::requested_device_from_policy(config.inference_device_policy),
            effective_device: OnnxExecutionDevice::Cpu,
            fallback_used: false,
            cuda_available: caps.cuda_available,
            reason: message.clone(),
            last_error: Some(message),
            model_id: Some(model_id.to_string()),
            updated_ms: Self::now_ms(),
        }
    }

    fn load_engine_for_config(
        model_info: &ModelInfo,
        model_path: &Path,
        config: &TranscriptionConfig,
    ) -> Result<(LoadedEngine, InferenceRuntimeStatus)> {
        let caps = detect_onnx_execution_capabilities();

        if !Self::is_onnx_engine(&model_info.engine_type) {
            let params =
                Self::onnx_execution_params_for_device(config, OnnxExecutionDevice::Cpu, false);
            let loaded_engine = Self::load_engine_with_params(model_info, model_path, &params)?;
            let status = Self::build_success_runtime_status(
                config,
                &model_info.id,
                caps,
                OnnxExecutionDevice::Cpu,
                false,
                "Whisper backend ignores ONNX execution provider settings".to_string(),
                None,
            );
            return Ok((loaded_engine, status));
        }

        match config.inference_device_policy {
            InferenceDevicePolicy::Cpu => {
                let params =
                    Self::onnx_execution_params_for_device(config, OnnxExecutionDevice::Cpu, false);
                let loaded_engine = Self::load_engine_with_params(model_info, model_path, &params)?;
                let status = Self::build_success_runtime_status(
                    config,
                    &model_info.id,
                    caps,
                    OnnxExecutionDevice::Cpu,
                    false,
                    "CPU mode requested".to_string(),
                    None,
                );
                Ok((loaded_engine, status))
            }
            InferenceDevicePolicy::Gpu => {
                if !caps.cuda_available {
                    return Err(anyhow::anyhow!(
                        "GPU mode requested but ONNX Runtime CUDA provider is unavailable"
                    ));
                }
                let params =
                    Self::onnx_execution_params_for_device(config, OnnxExecutionDevice::Gpu, false);
                let loaded_engine = Self::load_engine_with_params(model_info, model_path, &params)?;
                let status = Self::build_success_runtime_status(
                    config,
                    &model_info.id,
                    caps,
                    OnnxExecutionDevice::Gpu,
                    false,
                    format!(
                        "GPU mode requested and initialized on CUDA device {}",
                        config.inference_gpu_device_id.max(0)
                    ),
                    None,
                );
                Ok((loaded_engine, status))
            }
            InferenceDevicePolicy::Auto => {
                if caps.cuda_available {
                    let gpu_params = Self::onnx_execution_params_for_device(
                        config,
                        OnnxExecutionDevice::Gpu,
                        false,
                    );
                    match Self::load_engine_with_params(model_info, model_path, &gpu_params) {
                        Ok(loaded_engine) => {
                            let status = Self::build_success_runtime_status(
                                config,
                                &model_info.id,
                                caps,
                                OnnxExecutionDevice::Gpu,
                                false,
                                format!(
                                    "Auto mode selected CUDA device {}",
                                    config.inference_gpu_device_id.max(0)
                                ),
                                None,
                            );
                            Ok((loaded_engine, status))
                        }
                        Err(gpu_error) => {
                            warn!(
                                "Auto mode GPU initialization failed for model {}: {}. Retrying CPU fallback.",
                                model_info.id, gpu_error
                            );
                            let cpu_params = Self::onnx_execution_params_for_device(
                                config,
                                OnnxExecutionDevice::Cpu,
                                false,
                            );
                            let loaded_engine =
                                Self::load_engine_with_params(model_info, model_path, &cpu_params)
                                    .map_err(|cpu_error| {
                                        anyhow::anyhow!(
                                            "Auto inference initialization failed. GPU attempt: {}; CPU fallback attempt: {}",
                                            gpu_error,
                                            cpu_error
                                        )
                                    })?;
                            let status = Self::build_success_runtime_status(
                                config,
                                &model_info.id,
                                caps,
                                OnnxExecutionDevice::Cpu,
                                true,
                                "Auto mode fell back to CPU after GPU initialization failure"
                                    .to_string(),
                                Some(gpu_error.to_string()),
                            );
                            Ok((loaded_engine, status))
                        }
                    }
                } else {
                    let cpu_params = Self::onnx_execution_params_for_device(
                        config,
                        OnnxExecutionDevice::Cpu,
                        false,
                    );
                    let loaded_engine =
                        Self::load_engine_with_params(model_info, model_path, &cpu_params)?;
                    let status = Self::build_success_runtime_status(
                        config,
                        &model_info.id,
                        caps,
                        OnnxExecutionDevice::Cpu,
                        false,
                        "Auto mode detected no CUDA provider; using CPU".to_string(),
                        None,
                    );
                    Ok((loaded_engine, status))
                }
            }
        }
    }

    fn set_inference_runtime_status(&self, mut status: InferenceRuntimeStatus) {
        status.updated_ms = Self::now_ms();
        let mut current = self.shared.inference_runtime_status.lock().unwrap();
        *current = status;
    }

    fn set_inference_runtime_status_shared(
        shared: &Arc<SharedState>,
        mut status: InferenceRuntimeStatus,
    ) {
        status.updated_ms = Self::now_ms();
        let mut current = shared.inference_runtime_status.lock().unwrap();
        *current = status;
    }
}
impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::Relaxed);
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            if let Err(err) = handle.join() {
                warn!("Transcription watcher thread join failed: {:?}", err);
            }
        }
    }
}

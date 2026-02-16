use crate::audio_toolkit::{apply_custom_words, filter_transcription_output};
use crate::managers::model::{EngineType, ModelManager};
use crate::settings::{ModelUnloadTimeout, Settings};
use anyhow::Result;
use log::{debug, error, info, warn};
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
    TranscriptionEngine,
};

enum LoadedEngine {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
    Moonshine(MoonshineEngine),
    SenseVoice(SenseVoiceEngine),
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<Option<LoadedEngine>>>,
    model_manager: Arc<ModelManager>,
    settings: Settings,
    current_model_id: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
}

impl TranscriptionManager {
    pub fn new(model_manager: Arc<ModelManager>) -> Result<Self> {
        let settings = Settings::new();
        
        let manager = Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager,
            settings: settings.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            )),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
        };

        {
            let manager_cloned = manager.clone();
            let settings_cloned = settings;
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10));

                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let timeout_seconds = settings_cloned.model_unload_timeout().to_seconds();

                    if let Some(limit_seconds) = timeout_seconds {
                        if settings_cloned.model_unload_timeout() == ModelUnloadTimeout::Immediately {
                            continue;
                        }

                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;

                        if now_ms.saturating_sub(last) > limit_seconds * 1000 {
                            if manager_cloned.is_model_loaded() {
                                debug!("Unloading model due to inactivity");
                                let _ = manager_cloned.unload_model();
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down");
            });
            *manager.watcher_handle.lock().unwrap() = Some(handle);
        }

        Ok(manager)
    }

    pub fn is_model_loaded(&self) -> bool {
        let engine = self.engine.lock().unwrap();
        engine.is_some()
    }

    pub fn unload_model(&self) -> Result<()> {
        debug!("Unloading model");

        {
            let mut engine = self.engine.lock().unwrap();
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
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = None;
        }

        debug!("Model unloaded");
        Ok(())
    }

    pub fn maybe_unload_immediately(&self, context: &str) {
        if self.settings.model_unload_timeout() == ModelUnloadTimeout::Immediately && self.is_model_loaded() {
            info!("Immediately unloading model after {}", context);
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

        let model_path = self.model_manager.get_model_path(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model path not found"))?;

        let loaded_engine = match model_info.engine_type {
            EngineType::Whisper => {
                let mut engine = WhisperEngine::new();
                engine.load_model(&model_path)?;
                LoadedEngine::Whisper(engine)
            }
            EngineType::Parakeet => {
                let mut engine = ParakeetEngine::new();
                engine.load_model_with_params(&model_path, ParakeetModelParams::int8())?;
                LoadedEngine::Parakeet(engine)
            }
            EngineType::Moonshine => {
                let mut engine = MoonshineEngine::new();
                engine.load_model_with_params(
                    &model_path,
                    MoonshineModelParams::variant(ModelVariant::Base),
                )?;
                LoadedEngine::Moonshine(engine)
            }
            EngineType::SenseVoice => {
                let mut engine = SenseVoiceEngine::new();
                engine.load_model_with_params(&model_path, SenseVoiceModelParams::int8())?;
                LoadedEngine::SenseVoice(engine)
            }
        };

        {
            let mut engine = self.engine.lock().unwrap();
            *engine = Some(loaded_engine);
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = Some(model_id.to_string());
        }

        info!("Model {} loaded successfully", model_id);
        Ok(())
    }

    pub fn initiate_model_load(&self) {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading || self.is_model_loaded() {
            return;
        }

        *is_loading = true;
        let manager = self.clone();
        drop(is_loading);

        thread::spawn(move || {
            let selected_model = manager.model_manager.get_current_model();
            if selected_model.is_empty() {
                warn!("No model selected");
                let mut is_loading = manager.is_loading.lock().unwrap();
                *is_loading = false;
                manager.loading_condvar.notify_all();
                return;
            }

            if let Err(e) = manager.load_model(&selected_model) {
                error!("Failed to load model {}: {}", selected_model, e);
            }

            let mut is_loading = manager.is_loading.lock().unwrap();
            *is_loading = false;
            manager.loading_condvar.notify_all();
        });
    }

    pub fn transcribe(&self, samples: Vec<f32>) -> Result<String> {
        self.update_activity();

        let model_id = {
            let current = self.current_model_id.lock().unwrap();
            current.clone()
        };

        if model_id.is_none() || !self.is_model_loaded() {
            self.initiate_model_load();
            
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }
        }

        let engine = self.engine.lock().unwrap();
        let loaded_engine = engine.as_ref().ok_or_else(|| anyhow::anyhow!("No engine loaded"))?;

        let settings = &self.settings;
        let language = settings.selected_language();
        let translate = settings.translate_to_english();

        let result = match loaded_engine {
            LoadedEngine::Whisper(e) => {
                let mut params = WhisperInferenceParams::default();
                if language != "auto" {
                    params.language = language.clone();
                }
                params.translate = translate;
                e.transcribe_with_params(&samples, params)
            }
            LoadedEngine::Parakeet(e) => e.transcribe(&samples),
            LoadedEngine::Moonshine(e) => e.transcribe(&samples),
            LoadedEngine::SenseVoice(e) => e.transcribe(&samples),
        };

        drop(engine);

        let mut text = result?;

        let custom_words = settings.custom_words();
        if !custom_words.is_empty() {
            text = apply_custom_words(&text, &custom_words);
        }

        let threshold = settings.word_correction_threshold();
        text = filter_transcription_output(&text, threshold);

        self.maybe_unload_immediately("transcription");

        Ok(text)
    }

    pub fn get_model_load_status(&self) -> (bool, bool, Option<String>) {
        let is_loading = *self.is_loading.lock().unwrap();
        let is_loaded = self.is_model_loaded();
        let current_model = self.current_model_id.lock().unwrap().clone();
        (is_loading, is_loaded, current_model)
    }

    fn update_activity(&self) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.last_activity.store(now, Ordering::Relaxed);
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::Relaxed);
    }
}

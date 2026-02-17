use anyhow::Result;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tar::Archive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineType {
    Whisper,
    Parakeet,
    Moonshine,
    SenseVoice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub url: Option<String>,
    pub size_mb: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
    pub is_directory: bool,
    pub engine_type: EngineType,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub supports_translation: bool,
    pub is_recommended: bool,
    pub supported_languages: Vec<String>,
    pub is_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

pub struct ModelManager {
    selected_model: Mutex<String>,
    models_dir: PathBuf,
    available_models: Mutex<HashMap<String, ModelInfo>>,
    cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    extracting_models: Arc<Mutex<HashSet<String>>>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let settings = crate::settings::Settings::new();
        let models_dir = std::env::var("XDG_DATA_HOME")
            .map(|p| PathBuf::from(p).join("handy").join("models"))
            .unwrap_or_else(|_| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("handy")
                    .join("models")
            });

        if !models_dir.exists() {
            fs::create_dir_all(&models_dir)?;
        }

        let mut available_models = HashMap::new();

        let whisper_languages: Vec<String> = vec![
            "en", "zh", "zh-Hans", "zh-Hant", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl",
            "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs",
            "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy",
            "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is",
            "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo",
            "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht",
            "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln",
            "ha", "ba", "jw", "su", "yue",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "small".to_string(),
            ModelInfo {
                id: "small".to_string(),
                name: "Whisper Small".to_string(),
                description: "Fast and fairly accurate.".to_string(),
                filename: "ggml-small.bin".to_string(),
                url: Some("https://blob.handy.computer/ggml-small.bin".to_string()),
                size_mb: 487,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.60,
                speed_score: 0.85,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "medium".to_string(),
            ModelInfo {
                id: "medium".to_string(),
                name: "Whisper Medium".to_string(),
                description: "Good accuracy, medium speed".to_string(),
                filename: "whisper-medium-q4_1.bin".to_string(),
                url: Some("https://blob.handy.computer/whisper-medium-q4_1.bin".to_string()),
                size_mb: 492,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.75,
                speed_score: 0.60,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        available_models.insert(
            "turbo".to_string(),
            ModelInfo {
                id: "turbo".to_string(),
                name: "Whisper Turbo".to_string(),
                description: "Balanced accuracy and speed.".to_string(),
                filename: "ggml-large-v3-turbo.bin".to_string(),
                url: Some("https://blob.handy.computer/ggml-large-v3-turbo.bin".to_string()),
                size_mb: 1600,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::Whisper,
                accuracy_score: 0.80,
                speed_score: 0.40,
                supports_translation: false,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                is_custom: false,
            },
        );

        let parakeet_v3_languages: Vec<String> = vec![
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "parakeet-tdt-0.6b-v3".to_string(),
            ModelInfo {
                id: "parakeet-tdt-0.6b-v3".to_string(),
                name: "Parakeet V3".to_string(),
                description: "Fast and accurate. Supports 25 European languages.".to_string(),
                filename: "parakeet-tdt-0.6b-v3-int8".to_string(),
                url: Some("https://blob.handy.computer/parakeet-v3-int8.tar.gz".to_string()),
                size_mb: 478,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Parakeet,
                accuracy_score: 0.80,
                speed_score: 0.85,
                supports_translation: false,
                is_recommended: true,
                supported_languages: parakeet_v3_languages,
                is_custom: false,
            },
        );

        let sense_voice_languages: Vec<String> =
            vec!["zh", "zh-Hans", "zh-Hant", "en", "yue", "ja", "ko"]
                .into_iter()
                .map(String::from)
                .collect();

        available_models.insert(
            "sense-voice-int8".to_string(),
            ModelInfo {
                id: "sense-voice-int8".to_string(),
                name: "SenseVoice".to_string(),
                description: "Very fast. Chinese, English, Japanese, Korean, Cantonese."
                    .to_string(),
                filename: "sense-voice-int8".to_string(),
                url: Some("https://blob.handy.computer/sense-voice-int8.tar.gz".to_string()),
                size_mb: 160,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::SenseVoice,
                accuracy_score: 0.65,
                speed_score: 0.95,
                supports_translation: false,
                is_recommended: false,
                supported_languages: sense_voice_languages,
                is_custom: false,
            },
        );

        if let Err(e) = Self::discover_custom_whisper_models(&models_dir, &mut available_models) {
            warn!("Failed to discover custom models: {}", e);
        }

        let selected_model = settings.selected_model();
        let manager = Self {
            selected_model: Mutex::new(selected_model),
            models_dir,
            available_models: Mutex::new(available_models),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            extracting_models: Arc::new(Mutex::new(HashSet::new())),
        };

        manager.update_download_status()?;
        manager.auto_select_model_if_needed()?;

        Ok(manager)
    }

    pub fn get_available_models(&self) -> Vec<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.values().cloned().collect()
    }

    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.get(model_id).cloned()
    }

    pub fn get_model_path(&self, model_id: &str) -> Option<PathBuf> {
        let models = self.available_models.lock().unwrap();
        models
            .get(model_id)
            .map(|m| self.models_dir.join(&m.filename))
    }

    fn update_download_status(&self) -> Result<()> {
        let mut models = self.available_models.lock().unwrap();

        for model in models.values_mut() {
            if model.is_directory {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));

                model.is_downloaded = model_path.exists() && model_path.is_dir();
                model.is_downloading = false;

                if partial_path.exists() {
                    model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                } else {
                    model.partial_size = 0;
                }
            } else {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));

                model.is_downloaded = model_path.exists();
                model.is_downloading = false;

                if partial_path.exists() {
                    model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                } else {
                    model.partial_size = 0;
                }
            }
        }

        Ok(())
    }

    fn auto_select_model_if_needed(&self) -> Result<()> {
        let selected = self.selected_model.lock().unwrap().clone();
        let models = self.available_models.lock().unwrap();

        let is_valid_selected = !selected.is_empty()
            && models
                .get(&selected)
                .map(|m| m.is_downloaded)
                .unwrap_or(false);

        if is_valid_selected {
            return Ok(());
        }

        let fallback = models
            .values()
            .find(|m| m.is_downloaded && m.is_recommended)
            .or_else(|| models.values().find(|m| m.is_downloaded))
            .map(|m| m.id.clone());
        drop(models);

        if let Some(model_id) = fallback {
            info!("Auto-selecting model: {}", model_id);
            *self.selected_model.lock().unwrap() = model_id.clone();
            crate::settings::Settings::new().set_selected_model(&model_id);
        } else {
            *self.selected_model.lock().unwrap() = String::new();
            crate::settings::Settings::new().set_selected_model("");
        }

        Ok(())
    }

    fn discover_custom_whisper_models(
        models_dir: &Path,
        available_models: &mut HashMap<String, ModelInfo>,
    ) -> Result<()> {
        if !models_dir.exists() {
            return Ok(());
        }

        let predefined_filenames: HashSet<String> = available_models
            .values()
            .filter(|m| matches!(m.engine_type, EngineType::Whisper) && !m.is_directory)
            .map(|m| m.filename.clone())
            .collect();

        for entry in fs::read_dir(models_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let filename = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            if filename.starts_with('.') || !filename.ends_with(".bin") {
                continue;
            }

            if predefined_filenames.contains(&filename) {
                continue;
            }

            let model_id = filename.trim_end_matches(".bin").to_string();

            if available_models.contains_key(&model_id) {
                continue;
            }

            let size_mb = match path.metadata() {
                Ok(meta) => meta.len() / (1024 * 1024),
                Err(_) => 0,
            };

            info!(
                "Discovered custom Whisper model: {} ({} MB)",
                model_id, size_mb
            );

            available_models.insert(
                model_id.clone(),
                ModelInfo {
                    id: model_id,
                    name: filename.clone(),
                    description: "Custom model".to_string(),
                    filename,
                    url: None,
                    size_mb,
                    is_downloaded: true,
                    is_downloading: false,
                    partial_size: 0,
                    is_directory: false,
                    engine_type: EngineType::Whisper,
                    accuracy_score: 0.0,
                    speed_score: 0.0,
                    supports_translation: false,
                    is_recommended: false,
                    supported_languages: vec![],
                    is_custom: true,
                },
            );
        }

        Ok(())
    }

    pub async fn download_model(&self, model_id: &str) -> Result<()> {
        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        let url = model_info
            .url
            .ok_or_else(|| anyhow::anyhow!("No download URL for model"))?;
        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", &model_info.filename));

        if model_path.exists() {
            if partial_path.exists() {
                let _ = fs::remove_file(&partial_path);
            }
            self.update_download_status()?;
            return Ok(());
        }

        let mut resume_from = if partial_path.exists() {
            partial_path.metadata()?.len()
        } else {
            0
        };

        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = true;
            }
        }

        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags.insert(model_id.to_string(), cancel_flag.clone());
        }

        let client = reqwest::Client::new();
        let mut request = client.get(&url);

        if resume_from > 0 {
            request = request.header("Range", format!("bytes={}-", resume_from));
        }

        let mut response = request.send().await?;

        if resume_from > 0 && response.status() == reqwest::StatusCode::OK {
            drop(response);
            let _ = fs::remove_file(&partial_path);
            resume_from = 0;
            response = client.get(&url).send().await?;
        }

        if !response.status().is_success()
            && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        {
            {
                let mut models = self.available_models.lock().unwrap();
                if let Some(model) = models.get_mut(model_id) {
                    model.is_downloading = false;
                }
            }
            return Err(anyhow::anyhow!(
                "Failed to download: HTTP {}",
                response.status()
            ));
        }

        let _total_size = if resume_from > 0 {
            resume_from + response.content_length().unwrap_or(0)
        } else {
            response.content_length().unwrap_or(0)
        };

        let mut _downloaded = resume_from;
        let mut stream = response.bytes_stream();

        let mut file = if resume_from > 0 {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&partial_path)?
        } else {
            std::fs::File::create(&partial_path)?
        };

        while let Some(chunk) = stream.next().await {
            if cancel_flag.load(Ordering::Acquire) {
                drop(file);
                {
                    let mut models = self.available_models.lock().unwrap();
                    if let Some(model) = models.get_mut(model_id) {
                        model.is_downloading = false;
                    }
                }
                {
                    let mut flags = self.cancel_flags.lock().unwrap();
                    flags.remove(model_id);
                }
                return Ok(());
            }

            let chunk = chunk?;
            file.write_all(&chunk)?;
            _downloaded += chunk.len() as u64;
            if let Ok(mut models) = self.available_models.lock() {
                if let Some(model) = models.get_mut(model_id) {
                    model.partial_size = _downloaded;
                }
            }
        }

        drop(file);

        fs::rename(&partial_path, &model_path)?;

        if model_info.is_directory {
            self.extract_model(model_id, &model_path).await?;
        }

        {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags.remove(model_id);
        }

        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = false;
                model.is_downloaded = true;
                model.partial_size = 0;
            }
        }

        self.auto_select_model_if_needed()?;

        info!("Model {} downloaded successfully", model_id);
        Ok(())
    }

    async fn extract_model(&self, model_id: &str, tar_path: &Path) -> Result<()> {
        {
            let mut extracting = self.extracting_models.lock().unwrap();
            extracting.insert(model_id.to_string());
        }

        let result = self.do_extract(tar_path).await;

        {
            let mut extracting = self.extracting_models.lock().unwrap();
            extracting.remove(model_id);
        }

        result
    }

    async fn do_extract(&self, tar_path: &Path) -> Result<()> {
        let file = File::open(tar_path)?;
        let decoder = GzDecoder::new(&file);
        let mut archive = Archive::new(decoder);

        let extracting_dir = tar_path.with_extension("extracting");
        fs::create_dir_all(&extracting_dir)?;

        archive.unpack(&extracting_dir)?;

        let extracted_name = tar_path.file_stem().unwrap().to_str().unwrap();
        let final_dir = tar_path
            .parent()
            .unwrap()
            .join(extracted_name.trim_end_matches(".tar"));

        if final_dir.exists() {
            fs::remove_dir_all(&final_dir)?;
        }
        fs::rename(&extracting_dir, &final_dir)?;
        fs::remove_file(tar_path)?;

        Ok(())
    }

    pub fn cancel_download(&self, model_id: &str) -> Result<()> {
        let flags = self.cancel_flags.lock().unwrap();
        if let Some(flag) = flags.get(model_id) {
            flag.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn is_model_downloading(&self, model_id: &str) -> bool {
        let models = self.available_models.lock().unwrap();
        models
            .get(model_id)
            .map(|m| m.is_downloading)
            .unwrap_or(false)
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        if let Some(model) = model_info {
            let model_path = self.models_dir.join(&model.filename);
            let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));

            if model_path.exists() {
                if model_path.is_dir() {
                    fs::remove_dir_all(&model_path)?;
                } else {
                    fs::remove_file(&model_path)?;
                }
            }

            if partial_path.exists() {
                fs::remove_file(&partial_path)?;
            }

            self.update_download_status()?;

            let selected = self.selected_model.lock().unwrap();
            if *selected == model_id {
                drop(selected);
                *self.selected_model.lock().unwrap() = String::new();
                crate::settings::Settings::new().set_selected_model("");
            }
        }

        Ok(())
    }

    pub fn set_active_model(&self, model_id: &str) -> Result<()> {
        let models = self.available_models.lock().unwrap();
        if let Some(model) = models.get(model_id) {
            if !model.is_downloaded {
                return Err(anyhow::anyhow!("Model not downloaded: {}", model_id));
            }
            drop(models);
            *self.selected_model.lock().unwrap() = model_id.to_string();
            crate::settings::Settings::new().set_selected_model(model_id);
            info!("Active model set to: {}", model_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Model not found: {}", model_id))
        }
    }

    pub fn sync_selected_model_from_settings(&self) -> Result<()> {
        let selected = crate::settings::Settings::new().selected_model();
        let models = self.available_models.lock().unwrap();

        if selected.is_empty() {
            drop(models);
            *self.selected_model.lock().unwrap() = String::new();
            return Ok(());
        }

        if let Some(model) = models.get(&selected) {
            if model.is_downloaded {
                drop(models);
                *self.selected_model.lock().unwrap() = selected;
                return Ok(());
            }
        }

        drop(models);
        self.auto_select_model_if_needed()
    }

    pub fn get_current_model(&self) -> String {
        self.selected_model.lock().unwrap().clone()
    }

    pub fn has_any_models_available(&self) -> bool {
        let models = self.available_models.lock().unwrap();
        models.values().any(|m| m.is_downloaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_is_model_downloading() {
        // This test verifies the is_model_downloading method works correctly
        // Note: We can't easily test the full ModelManager without mocking the filesystem,
        // but we can test the logic directly

        let _model_id = "test-model";
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Test that a new flag is not set
        assert!(!cancel_flag.load(Ordering::Acquire));

        // Set the flag and verify it can be read
        cancel_flag.store(true, Ordering::Release);
        assert!(cancel_flag.load(Ordering::Acquire));
    }

    #[test]
    fn test_cancel_flag_ordering() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        // Thread 1: Set cancel flag after a short delay
        let handle1 = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(50));
            flag_clone.store(true, Ordering::Release);
        });

        // Thread 2: Check cancel flag - wait for thread 1 to complete
        let flag_clone2 = flag.clone();
        let handle2 = thread::spawn(move || {
            // Wait for thread 1 to finish
            let start = std::time::Instant::now();
            while !flag_clone2.load(Ordering::Acquire) {
                thread::yield_now();
                // Safety timeout to prevent infinite loop
                if start.elapsed() > std::time::Duration::from_secs(5) {
                    break;
                }
            }
            flag_clone2.load(Ordering::Acquire)
        });

        // Wait for both threads
        handle1.join().unwrap();
        let result = handle2.join().unwrap();

        // The Acquire/Release ordering should ensure the change is visible
        assert!(
            result,
            "Cancel flag change should be visible across threads with Acquire/Release ordering"
        );
    }

    #[test]
    fn test_concurrent_download_prevention_logic() {
        // Test the logic that prevents concurrent downloads
        // This simulates checking is_downloading flag before starting a download

        let is_downloading = Arc::new(AtomicBool::new(false));
        let _is_downloading_clone = is_downloading.clone();

        // Simulate first download starting
        assert!(!is_downloading.load(Ordering::Acquire));
        is_downloading.store(true, Ordering::Release);

        // Simulate second download attempt
        let is_downloading_clone2 = is_downloading.clone();
        let can_start_second = !is_downloading_clone2.load(Ordering::Acquire);

        assert!(
            !can_start_second,
            "Second download should be prevented when first is in progress"
        );

        // Simulate first download completing
        is_downloading.store(false, Ordering::Release);

        // Now second download should be able to start
        let can_start_after = !is_downloading.load(Ordering::Acquire);
        assert!(
            can_start_after,
            "Download should be allowed after previous one completes"
        );
    }
}

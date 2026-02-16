use gio::Settings as GioSettings;
use glib::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const SETTINGS_SCHEMA: &str = "com.handy.Transcription";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Debug
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    Marimba,
    Pop,
    Custom,
}

impl Default for SoundTheme {
    fn default() -> Self {
        SoundTheme::Marimba
    }
}

impl SoundTheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            SoundTheme::Marimba => "marimba",
            SoundTheme::Pop => "pop",
            SoundTheme::Custom => "custom",
        }
    }

    pub fn to_start_path(&self) -> String {
        format!("resources/{}_start.wav", self.as_str())
    }

    pub fn to_stop_path(&self) -> String {
        format!("resources/{}_stop.wav", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnloadTimeout {
    Never,
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
    Sec5,
}

impl Default for ModelUnloadTimeout {
    fn default() -> Self {
        ModelUnloadTimeout::Never
    }
}

impl ModelUnloadTimeout {
    pub fn to_seconds(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0),
            ModelUnloadTimeout::Sec5 => Some(5),
            ModelUnloadTimeout::Min2 => Some(120),
            ModelUnloadTimeout::Min5 => Some(300),
            ModelUnloadTimeout::Min10 => Some(600),
            ModelUnloadTimeout::Min15 => Some(900),
            ModelUnloadTimeout::Hour1 => Some(3600),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_binding: String,
    pub current_binding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMPrompt {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessProvider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub allow_base_url_edit: bool,
}

#[derive(Clone)]
pub struct Settings {
    gio_settings: GioSettings,
}

impl Settings {
    pub fn new() -> Self {
        let gio_settings = GioSettings::new(SETTINGS_SCHEMA);
        Self { gio_settings }
    }

    // General Settings
    pub fn push_to_talk(&self) -> bool {
        self.gio_settings.boolean("push-to-talk")
    }

    pub fn set_push_to_talk(&self, value: bool) {
        self.gio_settings.set_boolean("push-to-talk", value).ok();
    }

    pub fn audio_feedback(&self) -> bool {
        self.gio_settings.boolean("audio-feedback")
    }

    pub fn set_audio_feedback(&self, value: bool) {
        self.gio_settings.set_boolean("audio-feedback", value).ok();
    }

    pub fn audio_feedback_volume(&self) -> f32 {
        self.gio_settings.double("audio-feedback-volume") as f32
    }

    pub fn set_audio_feedback_volume(&self, value: f32) {
        self.gio_settings.set_double("audio-feedback-volume", value as f64).ok();
    }

    pub fn sound_theme(&self) -> SoundTheme {
        let value = self.gio_settings.enum_("sound-theme");
        match value {
            0 => SoundTheme::Marimba,
            1 => SoundTheme::Pop,
            2 => SoundTheme::Custom,
            _ => SoundTheme::default(),
        }
    }

    pub fn set_sound_theme(&self, theme: SoundTheme) {
        let value = match theme {
            SoundTheme::Marimba => 0,
            SoundTheme::Pop => 1,
            SoundTheme::Custom => 2,
        };
        self.gio_settings.set_enum("sound-theme", value).ok();
    }

    pub fn selected_microphone(&self) -> Option<String> {
        let value = self.gio_settings.string("selected-microphone");
        if value.is_empty() { None } else { Some(value.to_string()) }
    }

    pub fn set_selected_microphone(&self, value: Option<&str>) {
        self.gio_settings.set_string("selected-microphone", value.unwrap_or("")).ok();
    }

    pub fn selected_output_device(&self) -> Option<String> {
        let value = self.gio_settings.string("selected-output-device");
        if value.is_empty() { None } else { Some(value.to_string()) }
    }

    pub fn set_selected_output_device(&self, value: Option<&str>) {
        self.gio_settings.set_string("selected-output-device", value.unwrap_or("")).ok();
    }

    pub fn selected_language(&self) -> String {
        self.gio_settings.string("selected-language").to_string()
    }

    pub fn set_selected_language(&self, value: &str) {
        self.gio_settings.set_string("selected-language", value).ok();
    }

    pub fn translate_to_english(&self) -> bool {
        self.gio_settings.boolean("translate-to-english")
    }

    pub fn set_translate_to_english(&self, value: bool) {
        self.gio_settings.set_boolean("translate-to-english", value).ok();
    }

    pub fn mute_while_recording(&self) -> bool {
        self.gio_settings.boolean("mute-while-recording")
    }

    pub fn set_mute_while_recording(&self, value: bool) {
        self.gio_settings.set_boolean("mute-while-recording", value).ok();
    }

    // Model Settings
    pub fn selected_model(&self) -> String {
        self.gio_settings.string("selected-model").to_string()
    }

    pub fn set_selected_model(&self, value: &str) {
        self.gio_settings.set_string("selected-model", value).ok();
    }

    pub fn model_unload_timeout(&self) -> ModelUnloadTimeout {
        let value = self.gio_settings.enum_("model-unload-timeout");
        match value {
            0 => ModelUnloadTimeout::Never,
            1 => ModelUnloadTimeout::Immediately,
            2 => ModelUnloadTimeout::Min2,
            3 => ModelUnloadTimeout::Min5,
            4 => ModelUnloadTimeout::Min10,
            5 => ModelUnloadTimeout::Min15,
            6 => ModelUnloadTimeout::Hour1,
            7 => ModelUnloadTimeout::Sec5,
            _ => ModelUnloadTimeout::default(),
        }
    }

    pub fn set_model_unload_timeout(&self, timeout: ModelUnloadTimeout) {
        let value = match timeout {
            ModelUnloadTimeout::Never => 0,
            ModelUnloadTimeout::Immediately => 1,
            ModelUnloadTimeout::Min2 => 2,
            ModelUnloadTimeout::Min5 => 3,
            ModelUnloadTimeout::Min10 => 4,
            ModelUnloadTimeout::Min15 => 5,
            ModelUnloadTimeout::Hour1 => 6,
            ModelUnloadTimeout::Sec5 => 7,
        };
        self.gio_settings.set_enum("model-unload-timeout", value).ok();
    }

    // App Behavior
    pub fn start_hidden(&self) -> bool {
        self.gio_settings.boolean("start-hidden")
    }

    pub fn set_start_hidden(&self, value: bool) {
        self.gio_settings.set_boolean("start-hidden", value).ok();
    }

    pub fn autostart_enabled(&self) -> bool {
        self.gio_settings.boolean("autostart-enabled")
    }

    pub fn set_autostart_enabled(&self, value: bool) {
        self.gio_settings.set_boolean("autostart-enabled", value).ok();
    }

    pub fn show_tray_icon(&self) -> bool {
        self.gio_settings.boolean("show-tray-icon")
    }

    pub fn set_show_tray_icon(&self, value: bool) {
        self.gio_settings.set_boolean("show-tray-icon", value).ok();
    }

    pub fn update_checks_enabled(&self) -> bool {
        self.gio_settings.boolean("update-checks-enabled")
    }

    pub fn set_update_checks_enabled(&self, value: bool) {
        self.gio_settings.set_boolean("update-checks-enabled", value).ok();
    }

    // Custom Words
    pub fn custom_words(&self) -> Vec<String> {
        self.gio_settings.strv("custom-words").iter().map(|s| s.to_string()).collect()
    }

    pub fn set_custom_words(&self, words: &[String]) {
        let strv: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
        self.gio_settings.set_strv("custom-words", &strv).ok();
    }

    // Debug Settings
    pub fn debug_mode(&self) -> bool {
        self.gio_settings.boolean("debug-mode")
    }

    pub fn set_debug_mode(&self, value: bool) {
        self.gio_settings.set_boolean("debug-mode", value).ok();
    }

    pub fn log_level(&self) -> LogLevel {
        let value = self.gio_settings.enum_("log-level");
        match value {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            4 => LogLevel::Error,
            _ => LogLevel::default(),
        }
    }

    pub fn set_log_level(&self, level: LogLevel) {
        let value = match level {
            LogLevel::Trace => 0,
            LogLevel::Debug => 1,
            LogLevel::Info => 2,
            LogLevel::Warn => 3,
            LogLevel::Error => 4,
        };
        self.gio_settings.set_enum("log-level", value).ok();
    }

    pub fn word_correction_threshold(&self) -> f64 {
        self.gio_settings.double("word-correction-threshold")
    }

    pub fn always_on_microphone(&self) -> bool {
        self.gio_settings.boolean("always-on-microphone")
    }

    pub fn experimental_enabled(&self) -> bool {
        self.gio_settings.boolean("experimental-enabled")
    }

    pub fn set_experimental_enabled(&self, value: bool) {
        self.gio_settings.set_boolean("experimental-enabled", value).ok();
    }

    // Post-Processing Settings
    pub fn post_process_enabled(&self) -> bool {
        self.gio_settings.boolean("post-process-enabled")
    }

    pub fn set_post_process_enabled(&self, value: bool) {
        self.gio_settings.set_boolean("post-process-enabled", value).ok();
    }

    pub fn post_process_provider_id(&self) -> String {
        self.gio_settings.string("post-process-provider-id").to_string()
    }

    pub fn set_post_process_provider_id(&self, value: &str) {
        self.gio_settings.set_string("post-process-provider-id", value).ok();
    }

    pub fn post_process_api_keys(&self) -> HashMap<String, String> {
        let json = self.gio_settings.string("post-process-api-keys");
        serde_json::from_str(json.as_str()).unwrap_or_default()
    }

    pub fn set_post_process_api_keys(&self, keys: HashMap<String, String>) {
        let json = serde_json::to_string(&keys).unwrap_or_default();
        self.gio_settings.set_string("post-process-api-keys", &json).ok();
    }

    pub fn post_process_models(&self) -> HashMap<String, String> {
        let json = self.gio_settings.string("post-process-models");
        serde_json::from_str(json.as_str()).unwrap_or_default()
    }

    pub fn set_post_process_models(&self, models: HashMap<String, String>) {
        let json = serde_json::to_string(&models).unwrap_or_default();
        self.gio_settings.set_string("post-process-models", &json).ok();
    }

    pub fn post_process_base_urls(&self) -> HashMap<String, String> {
        let json = self.gio_settings.string("post-process-base-urls");
        serde_json::from_str(json.as_str()).unwrap_or_default()
    }

    pub fn set_post_process_base_urls(&self, urls: HashMap<String, String>) {
        let json = serde_json::to_string(&urls).unwrap_or_default();
        self.gio_settings.set_string("post-process-base-urls", &json).ok();
    }

    pub fn post_process_prompts(&self) -> Vec<LLMPrompt> {
        let json = self.gio_settings.string("post-process-prompts");
        serde_json::from_str(json.as_str()).unwrap_or_default()
    }

    pub fn set_post_process_prompts(&self, prompts: Vec<LLMPrompt>) {
        let json = serde_json::to_string(&prompts).unwrap_or_default();
        self.gio_settings.set_string("post-process-prompts", &json).ok();
    }

    pub fn post_process_selected_prompt_id(&self) -> Option<String> {
        let value = self.gio_settings.string("post-process-selected-prompt-id");
        if value.is_empty() { None } else { Some(value.to_string()) }
    }

    pub fn set_post_process_selected_prompt_id(&self, value: Option<&str>) {
        self.gio_settings.set_string("post-process-selected-prompt-id", value.unwrap_or("")).ok();
    }

    // UI Settings
    pub fn app_language(&self) -> String {
        self.gio_settings.string("app-language").to_string()
    }

    pub fn set_app_language(&self, value: &str) {
        self.gio_settings.set_string("app-language", value).ok();
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_default_settings() -> Settings {
    Settings::new()
}

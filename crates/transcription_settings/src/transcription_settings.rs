use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::RegisterSetting;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, RegisterSetting)]
#[serde(default)]
pub struct SpeechSettings {
    pub enabled: Option<bool>,
    pub model: Option<String>,
    pub ai_provider: Option<String>,

    pub threads: Option<usize>,
    pub language: Option<String>,
    pub start_sensitivity: Option<f32>,
    pub start_timeout: Option<u64>,
    pub stop_sensitivity: Option<f32>,
    pub stop_timeout: Option<u64>,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            enabled: None,
            model: None,
            ai_provider: None,

            threads: None,
            language: None,
            start_sensitivity: None,
            start_timeout: None,
            stop_sensitivity: None,
            stop_timeout: None,
        }
    }
}

impl settings::Settings for SpeechSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let content = content.speech.clone().unwrap();

        Self {
            enabled: content.enabled,
            model: content.model,
            ai_provider: content.ai_provider,

            threads: content.threads,
            language: content.language,
            start_sensitivity: content.start_sensitivity,
            start_timeout: content.start_timeout,
            stop_sensitivity: content.stop_sensitivity,
            stop_timeout: content.stop_timeout,
        }
    }
}

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

#[with_fallible_options]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, MergeFrom, Default)]
pub struct SpeechSettingsContent {
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

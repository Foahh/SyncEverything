use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum InferenceTier {
    EdgeFastPath,
    HostLocalOllama,
    CloudOpenAiCompat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct LlmEndpointConfig {
    pub tier: InferenceTier,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
}

impl Default for LlmEndpointConfig {
    fn default() -> Self {
        Self {
            tier: InferenceTier::HostLocalOllama,
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            model: "qwen3.5:4b".to_string(),
            api_key_env: None,
        }
    }
}

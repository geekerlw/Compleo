pub mod openai;

use tokio::sync::mpsc;

/// Request to the LLM
pub struct LlmRequest {
    pub system_prompt: String,
    pub current_context: String, // OCR text
    pub draft: Option<String>,   // User's draft in input field
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Reply,    // Generate a full reply
    Complete, // Complete user's draft
}

/// Configuration for the LLM backend
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl LlmConfig {
    /// Load config from environment variables
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("COMPLEO_API_KEY"))
            .map_err(|_| "Set OPENAI_API_KEY or COMPLEO_API_KEY environment variable".to_string())?;

        let model = std::env::var("COMPLEO_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".to_string());

        let base_url = std::env::var("COMPLEO_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        Ok(Self {
            api_key,
            model,
            base_url,
        })
    }
}

/// Generate a streaming response from the LLM.
/// Sends text chunks through the channel as they arrive.
/// Returns the full accumulated text when done.
pub async fn generate_stream(
    config: &LlmConfig,
    request: LlmRequest,
    tx: mpsc::UnboundedSender<String>,
) -> Result<String, String> {
    openai::stream_chat_completion(config, request, tx).await
}

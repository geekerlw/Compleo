pub mod openai;

use tokio::sync::mpsc;

/// Request to the LLM
pub struct LlmRequest {
    pub current_context: String, // OCR text + style context
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum Mode {
    Reply, // Generate a full reply
}

/// Configuration for the LLM backend
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
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

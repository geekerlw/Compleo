//! Background conversation distillation.
//! Uses LLM to extract structured messages from raw OCR text,
//! then generates embeddings for semantic search.

use crate::config::Config;
use crate::storage::Storage;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Distill prompt: extract structured messages from OCR text
const DISTILL_PROMPT: &str = r#"你是一个文本结构化工具。给定聊天 OCR 文本（带←→标记），请提取出每条消息的发送者和内容。

输入格式：每行前有 ←（对方）、→（用户）、·（系统消息）标记。

输出 JSON 数组，每项格式：
{"sender": "对方名字或other", "content": "消息内容", "is_user": false}
{"sender": "user", "content": "消息内容", "is_user": true}

规则：
- 忽略系统消息（·标记）和 UI 噪音
- sender 尽量用真实名字（如果 OCR 中有），否则用 "other"
- is_user: → 标记的是 true，← 标记的是 false
- 只输出 JSON 数组，不要其他文字"#;

/// Run one round of distillation on undistilled conversations.
/// Returns the number of conversations processed.
pub async fn run_distillation(storage: &Storage, config: &Config) -> usize {
    let conversations = storage.undistilled_conversations(5);
    if conversations.is_empty() {
        return 0;
    }

    log::info!("Distilling {} conversations", conversations.len());
    let client = Client::new();
    let mut processed = 0;

    for conv in &conversations {
        match distill_one(&client, config, &conv.ocr_text).await {
            Ok(messages) => {
                if !messages.is_empty() {
                    let msg_tuples: Vec<(String, String, bool)> = messages
                        .iter()
                        .map(|m| (m.sender.clone(), m.content.clone(), m.is_user))
                        .collect();

                    if let Err(e) = storage.save_distilled_messages(conv.id, &conv.app_name, &msg_tuples) {
                        log::error!("Failed to save distilled messages: {}", e);
                        continue;
                    }
                }
                let _ = storage.mark_distilled(conv.id);
                processed += 1;
            }
            Err(e) => {
                log::error!("Distillation failed for conv {}: {}", conv.id, e);
                // Mark as distilled anyway to avoid retrying forever
                let _ = storage.mark_distilled(conv.id);
            }
        }
    }

    log::info!("Distilled {} conversations", processed);
    processed
}

/// Generate embeddings for messages that don't have them yet.
/// Uses OpenAI embeddings API.
pub async fn run_embedding_generation(storage: &Storage, config: &Config) -> usize {
    let messages = storage.messages_without_embeddings(20);
    if messages.is_empty() {
        return 0;
    }

    log::info!("Generating embeddings for {} messages", messages.len());
    let client = Client::new();
    let mut processed = 0;

    // Batch messages for embedding (up to 20 at a time)
    let texts: Vec<&str> = messages.iter().map(|(_, _, content)| content.as_str()).collect();

    match generate_embeddings(&client, config, &texts).await {
        Ok(vectors) => {
            for (i, (msg_id, app_name, content)) in messages.iter().enumerate() {
                if i < vectors.len() {
                    if let Err(e) = storage.save_embedding(*msg_id, app_name, content, &vectors[i]) {
                        log::error!("Failed to save embedding: {}", e);
                        continue;
                    }
                    processed += 1;
                }
            }
        }
        Err(e) => {
            log::error!("Embedding generation failed: {}", e);
        }
    }

    log::info!("Generated {} embeddings", processed);
    processed
}

/// Embed a single query text for semantic search
pub async fn embed_query(config: &Config, text: &str) -> Result<Vec<f32>, String> {
    let client = Client::new();
    let vectors = generate_embeddings(&client, config, &[text]).await?;
    vectors.into_iter().next().ok_or_else(|| "No embedding returned".to_string())
}

// ========== Internal helpers ==========

#[derive(Deserialize)]
struct DistilledMsg {
    sender: String,
    content: String,
    is_user: bool,
}

async fn distill_one(
    client: &Client,
    config: &Config,
    ocr_text: &str,
) -> Result<Vec<DistilledMsg>, String> {
    #[derive(Serialize)]
    struct Req {
        model: String,
        messages: Vec<Msg>,
        temperature: f32,
    }
    #[derive(Serialize)]
    struct Msg { role: String, content: String }

    #[derive(Deserialize)]
    struct Resp { choices: Vec<Choice> }
    #[derive(Deserialize)]
    struct Choice { message: RMsg }
    #[derive(Deserialize)]
    struct RMsg { content: Option<String> }

    let url = format!("{}/chat/completions", config.base_url);
    let req = Req {
        model: config.model.clone(),
        messages: vec![
            Msg { role: "system".into(), content: DISTILL_PROMPT.into() },
            Msg { role: "user".into(), content: ocr_text.to_string() },
        ],
        temperature: 0.1,
    };

    let resp = client.post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("API error ({}): {}", status, &body[..body.len().min(200)]));
    }

    let body: Resp = resp.json().await.map_err(|e| format!("Parse error: {}", e))?;
    let content = body.choices.first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("[]");

    // Extract JSON array from content (might have markdown fences)
    let json_str = extract_json_array(content);
    let messages: Vec<DistilledMsg> = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| {
            log::warn!("Failed to parse distilled JSON: {}", e);
            vec![]
        });

    Ok(messages)
}

async fn generate_embeddings(
    client: &Client,
    config: &Config,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, String> {
    #[derive(Serialize)]
    struct Req { model: String, input: Vec<String> }

    #[derive(Deserialize)]
    struct Resp { data: Vec<EmbData> }
    #[derive(Deserialize)]
    struct EmbData { embedding: Vec<f32> }

    let url = format!("{}/embeddings", config.base_url);
    let req = Req {
        model: "text-embedding-3-small".to_string(),
        input: texts.iter().map(|t| t.to_string()).collect(),
    };

    let resp = client.post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Embedding API error ({}): {}", status, &body[..body.len().min(200)]));
    }

    let body: Resp = resp.json().await.map_err(|e| format!("Parse error: {}", e))?;
    Ok(body.data.into_iter().map(|d| d.embedding).collect())
}

/// Extract JSON array from content that might be wrapped in markdown fences
fn extract_json_array(content: &str) -> String {
    let trimmed = content.trim();

    // Try to find JSON array directly
    if trimmed.starts_with('[') {
        return trimmed.to_string();
    }

    // Look for ```json ... ``` block
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            return trimmed[start..=end].to_string();
        }
    }

    "[]".to_string()
}

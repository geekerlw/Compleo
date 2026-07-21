use super::{LlmConfig, LlmRequest};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    stream: bool,
    temperature: f32,
}

#[derive(Serialize, Clone)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Delta,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

const SYSTEM_PROMPT_REPLY: &str = r#"你是一个聊天回复助手。用户提供聊天截屏的 OCR 文本，请代替用户生成一条回复。

OCR 文本格式说明：
- 每行前面有标记表示消息来自哪一侧：
  - "←" = 左侧消息 = 对方发的
  - "→" = 右侧消息 = 用户自己发的
  - "·" = 居中内容 = 时间戳或系统消息
- 需要忽略的内容（不是聊天消息）：
  - 底部工具栏：Send、New Line、Please enter a message、Watermark、Group Check 等
  - QQ群右侧面板：群聊成员、群主、管理员、成员昵称列表
  - 图标文字：emoji符号、◎、②、④ 等
  - All Read、Unread 等状态标记

你的任务：
- 只关注真实的聊天消息（←和→标记的有意义的文字）
- 根据标记判断谁是对方（←），谁是用户（→）
- 找到对方最近的消息，代替用户生成回复
- 如果提供了"用户说话风格画像"，严格模仿该风格（用词、语气、长度）
- 如果提供了"用户之前的回复风格参考"，参考其用词习惯
- 语言与聊天一致，简洁自然，1-2 句话
- 只输出回复内容本身，不要加任何前缀或解释"#;

pub async fn stream_chat_completion(
    config: &LlmConfig,
    request: LlmRequest,
    tx: mpsc::UnboundedSender<String>,
) -> Result<String, String> {
    let client = Client::new();

    let system_prompt = SYSTEM_PROMPT_REPLY.to_string();

    let user_content = format!("以下是聊天截屏的 OCR 文本：\n\n{}", request.current_context);

    let messages = vec![
        Message {
            role: "system".to_string(),
            content: system_prompt,
        },
        Message {
            role: "user".to_string(),
            content: user_content,
        },
    ];

    let chat_request = ChatRequest {
        model: config.model.clone(),
        messages,
        stream: true,
        temperature: 0.7,
    };

    let url = format!("{}/chat/completions", config.base_url);

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&chat_request)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("API error ({}): {}", status, body));
    }

    // Stream SSE response
    let mut stream = response.bytes_stream();
    let mut full_content = String::new(); // Only non-thinking content (the actual reply)
    let mut buffer = String::new();
    let mut in_think = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete SSE lines
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[(line_end + 1)..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data.trim() == "[DONE]" {
                    return Ok(full_content.trim().to_string());
                }

                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                    for choice in &chunk.choices {
                        // Handle reasoning_content field (DeepSeek style)
                        if let Some(reasoning) = &choice.delta.reasoning_content {
                            if !reasoning.is_empty() {
                                let msg = serde_json::json!({"type": "thinking", "text": reasoning});
                                let _ = tx.send(msg.to_string());
                            }
                        }
                        // Handle content field (may contain <think> tags for MiniMax etc)
                        if let Some(content) = &choice.delta.content {
                            if !content.is_empty() {
                                emit_content(content, &mut in_think, &mut full_content, &tx);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(full_content.trim().to_string())
}

/// Process content that may contain <think>...</think> tags.
/// - Thinking parts → sent as {"type":"thinking"} to UI
/// - Non-thinking parts → sent as {"type":"content"} to UI AND appended to full_content
fn emit_content(
    content: &str,
    in_think: &mut bool,
    full_content: &mut String,
    tx: &mpsc::UnboundedSender<String>,
) {
    let mut remaining = content;

    while !remaining.is_empty() {
        if *in_think {
            if let Some(end_pos) = remaining.find("</think>") {
                let think_text = &remaining[..end_pos];
                if !think_text.is_empty() {
                    let msg = serde_json::json!({"type": "thinking", "text": think_text});
                    let _ = tx.send(msg.to_string());
                }
                *in_think = false;
                remaining = &remaining[(end_pos + 8)..];
            } else {
                // All remaining is thinking
                if !remaining.is_empty() {
                    let msg = serde_json::json!({"type": "thinking", "text": remaining});
                    let _ = tx.send(msg.to_string());
                }
                break;
            }
        } else {
            if let Some(start_pos) = remaining.find("<think>") {
                let text = &remaining[..start_pos];
                if !text.is_empty() {
                    full_content.push_str(text);
                    let msg = serde_json::json!({"type": "content", "text": text});
                    let _ = tx.send(msg.to_string());
                }
                *in_think = true;
                remaining = &remaining[(start_pos + 7)..];
            } else {
                // All remaining is normal content
                full_content.push_str(remaining);
                let msg = serde_json::json!({"type": "content", "text": remaining});
                let _ = tx.send(msg.to_string());
                break;
            }
        }
    }
}

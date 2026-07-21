//! Background conversation distillation and style profile generation.
//! Uses LLM to extract structured messages from raw OCR text.

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
                let _ = storage.mark_distilled(conv.id);
            }
        }
    }

    log::info!("Distilled {} conversations", processed);
    processed
}

// ========== Style Profile Distillation ==========

const STYLE_PROFILE_PROMPT: &str = r#"你是一个语言风格分析师。分析以下用户在聊天中的消息样本，提炼出一份简洁的"说话风格画像"。

要求：
- 分析用词偏好（口头禅、常用词、语气词）
- 分析句式特点（长短句、断句习惯）
- 分析语气特征（正式/随意、幽默/严肃、直接/委婉）
- 分析回复长度偏好
- 用中文输出，控制在 150 字以内
- 只输出风格描述，不要分析过程

输出格式示例：
"偏好短句回复，常用语气词'嗯'、'好的'、'行'。语气随意直接，不绕弯子。偶尔用网络用语和表情符号。工作相关话题会稍微正式，但整体轻松。回复通常 1-2 句话。""#;

/// Distill style profiles from user's message history.
/// Only re-distills if there are significantly more samples than last time.
pub async fn distill_style_profiles(storage: &Storage, config: &Config) {
    let apps = storage.apps_with_conversations();

    for app_name in &apps {
        let messages = storage.user_messages_for_app(app_name, 30);
        if messages.len() < 5 {
            continue;
        }

        let current_count = messages.len() as i64;
        let last_count = storage.style_profile_sample_count(app_name);

        // Only re-distill if we have 50%+ more samples
        if last_count > 0 && current_count < last_count * 3 / 2 {
            continue;
        }

        log::info!("Distilling style profile for {} ({} samples)", app_name, current_count);

        match generate_style_profile(config, &messages).await {
            Ok(profile) => {
                if let Err(e) = storage.save_style_profile(app_name, &profile, current_count) {
                    log::error!("Failed to save style profile: {}", e);
                } else {
                    log::info!("Style profile for {}: {}", app_name, &profile[..profile.len().min(60)]);
                }
            }
            Err(e) => {
                log::error!("Style profile distillation failed for {}: {}", app_name, e);
            }
        }
    }
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
    struct Req { model: String, messages: Vec<Msg>, temperature: f32 }
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

    let json_str = extract_json_array(content);
    let messages: Vec<DistilledMsg> = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| {
            log::warn!("Failed to parse distilled JSON: {}", e);
            vec![]
        });

    Ok(messages)
}

async fn generate_style_profile(config: &Config, messages: &[String]) -> Result<String, String> {
    #[derive(Serialize)]
    struct Req { model: String, messages: Vec<Msg>, temperature: f32 }
    #[derive(Serialize)]
    struct Msg { role: String, content: String }
    #[derive(Deserialize)]
    struct Resp { choices: Vec<Choice> }
    #[derive(Deserialize)]
    struct Choice { message: RMsg }
    #[derive(Deserialize)]
    struct RMsg { content: Option<String> }

    let samples = messages.iter()
        .take(20)
        .enumerate()
        .map(|(i, m)| format!("{}. {}", i + 1, m))
        .collect::<Vec<_>>()
        .join("\n");

    let client = Client::new();
    let url = format!("{}/chat/completions", config.base_url);
    let req = Req {
        model: config.model.clone(),
        messages: vec![
            Msg { role: "system".into(), content: STYLE_PROFILE_PROMPT.into() },
            Msg { role: "user".into(), content: format!("以下是用户的聊天消息样本：\n\n{}", samples) },
        ],
        temperature: 0.3,
    };

    let resp = client.post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }

    let body: Resp = resp.json().await.map_err(|e| format!("Parse error: {}", e))?;
    let content = body.choices.first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();

    // Strip <think> tags if present
    let result = if let Some(pos) = content.find("</think>") {
        content[(pos + 8)..].trim().to_string()
    } else {
        content
    };

    // Strip surrounding quotes
    let result = result.trim_matches('"').trim_matches('「').trim_matches('」').trim().to_string();
    Ok(result)
}

/// Extract JSON array from content that might be wrapped in markdown fences
fn extract_json_array(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.starts_with('[') {
        return trimmed.to_string();
    }
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            return trimmed[start..=end].to_string();
        }
    }
    "[]".to_string()
}

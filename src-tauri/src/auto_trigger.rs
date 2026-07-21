//! Chat app detection for input monitoring.

/// Chat apps we recognize and monitor for Enter-triggered learning.
const CHAT_APPS: &[&str] = &[
    "WeChat", "微信",
    "QQ",
    "DingTalk", "钉钉",
    "Slack",
    "Telegram",
    "Messages", "信息",
    "Lark", "飞书",
];

/// Check if the given app name is a recognized chat app.
pub fn is_chat_app(app_name: &str) -> bool {
    CHAT_APPS.iter().any(|&name| app_name.contains(name))
}

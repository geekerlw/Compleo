use std::path::PathBuf;

/// Result type for platform operations
pub type PlatformResult<T> = Result<T, PlatformError>;

/// Errors that can occur during platform operations
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("Screenshot capture failed: {0}")]
    CaptureError(String),
    #[error("OCR failed: {0}")]
    OcrError(String),
    #[error("Could not determine frontmost application")]
    NoFrontmostApp,
    #[error("Clipboard operation failed: {0}")]
    ClipboardError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// A screenshot captured from the screen
pub struct Screenshot {
    /// Path to the temporary PNG file
    pub path: PathBuf,
    /// Width in pixels (may be used for layout detection)
    #[allow(dead_code)]
    pub width: u32,
    /// Height in pixels (may be used for layout detection)
    #[allow(dead_code)]
    pub height: u32,
}

/// Platform abstraction trait
pub trait PlatformProvider: Send + Sync {
    /// Capture the chat area of the frontmost window
    fn capture_chat_area(&self) -> PlatformResult<Screenshot>;

    /// OCR the screenshot and return recognized text
    fn ocr(&self, screenshot: &Screenshot) -> PlatformResult<String>;

    /// Get the frontmost application's display name
    fn frontmost_app_name(&self) -> PlatformResult<String>;

    /// Read the focused input field text (via Accessibility API)
    #[allow(dead_code)]
    fn read_input_field(&self) -> PlatformResult<Option<String>>;

    /// Write text to the system clipboard
    fn set_clipboard(&self, text: &str) -> PlatformResult<()>;
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacOSProvider;

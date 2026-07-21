use super::{PlatformError, PlatformProvider, PlatformResult, Screenshot};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::dictionary::CFDictionaryGetValueIfPresent;
use std::path::PathBuf;
use std::process::Command;

pub struct MacOSProvider {
    temp_dir: PathBuf,
}

impl MacOSProvider {
    pub fn new() -> Self {
        let temp_dir = std::env::temp_dir().join("compleo");
        std::fs::create_dir_all(&temp_dir).ok();
        Self { temp_dir }
    }

    /// Per-app crop configuration.
    /// Returns (top_pct, left_pct, right_pct) — percentage of image to remove from each side.
    fn crop_config_for_app(app_name: &str) -> (f32, f32, f32) {
        // top%, left%, right%
        if app_name.contains("WeChat") || app_name.contains("微信") {
            (0.25, 0.30, 0.0)  // Left sidebar ~30%, top nav ~25%
        } else if app_name.contains("QQ") {
            (0.25, 0.25, 0.20) // Left sidebar ~25%, right member panel ~20%
        } else if app_name.contains("DingTalk") || app_name.contains("钉钉") {
            (0.25, 0.30, 0.0)  // Left sidebar ~30%
        } else if app_name.contains("Slack") {
            (0.0, 0.22, 0.0)   // Left channel list ~22%, no top crop (compact header)
        } else if app_name.contains("Telegram") {
            (0.0, 0.28, 0.0)   // Left chat list ~28%
        } else if app_name.contains("Lark") || app_name.contains("飞书") {
            (0.25, 0.25, 0.0)  // Left sidebar ~25%
        } else if app_name.contains("Messages") || app_name.contains("信息") {
            (0.0, 0.30, 0.0)   // Left conversation list ~30%
        } else {
            // Default: remove left 30% + top 25% (works for most chat apps)
            (0.25, 0.30, 0.0)
        }
    }

    /// Crop screenshot to only keep the chat area, using per-app layout config.
    fn crop_to_chat_area(&self, path: &PathBuf, app_name: &str) -> PlatformResult<()> {
        // Get image dimensions using sips
        let output = Command::new("sips")
            .args(["-g", "pixelHeight", "-g", "pixelWidth", path.to_str().unwrap()])
            .output()
            .map_err(|e| PlatformError::CaptureError(format!("sips failed: {}", e)))?;

        let info = String::from_utf8_lossy(&output.stdout);
        let height = info.lines()
            .find(|l| l.contains("pixelHeight"))
            .and_then(|l| l.split(':').last())
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let width = info.lines()
            .find(|l| l.contains("pixelWidth"))
            .and_then(|l| l.split(':').last())
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);

        if height == 0 || width == 0 {
            log::warn!("Could not get image dimensions, skipping crop");
            return Ok(());
        }

        let (top_pct, left_pct, right_pct) = Self::crop_config_for_app(app_name);

        let crop_top = (height as f32 * top_pct) as u32;
        let crop_left = (width as f32 * left_pct) as u32;
        let crop_right = (width as f32 * right_pct) as u32;
        let new_height = height - crop_top;
        let new_width = width - crop_left - crop_right;

        if new_height < 100 || new_width < 100 {
            log::warn!("Crop result too small, skipping");
            return Ok(());
        }

        let output = Command::new("sips")
            .args([
                "-c", &new_height.to_string(), &new_width.to_string(),
                "--cropOffset", &crop_top.to_string(), &crop_left.to_string(),
                path.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| PlatformError::CaptureError(format!("sips crop failed: {}", e)))?;

        if !output.status.success() {
            log::warn!("sips crop failed, using full screenshot");
        } else {
            log::info!("Cropped screenshot: {}x{} (top {}px, left {}px, right {}px) [{}]",
                new_width, new_height, crop_top, crop_left, crop_right, app_name);
        }

        Ok(())
    }

    /// Get the window ID of the frontmost window
    fn frontmost_window_id(&self) -> PlatformResult<u32> {
        use core_graphics::window::{
            copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
            kCGWindowListOptionOnScreenOnly,
        };

        let app_name = self.frontmost_app_name()?;

        let info = copy_window_info(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
        .ok_or_else(|| PlatformError::CaptureError("Failed to get window list".into()))?;

        let values = info.get_all_values();

        for dict_ptr in &values {
            let dict_ref = *dict_ptr as core_foundation::dictionary::CFDictionaryRef;
            if dict_ref.is_null() {
                continue;
            }

            // Get owner name
            let owner_key = CFString::new("kCGWindowOwnerName");
            let mut owner_value: *const std::ffi::c_void = std::ptr::null();
            let has_owner = unsafe {
                CFDictionaryGetValueIfPresent(
                    dict_ref,
                    owner_key.as_concrete_TypeRef() as *const std::ffi::c_void,
                    &mut owner_value,
                )
            };

            if has_owner == 0 || owner_value.is_null() {
                continue;
            }

            let owner_str = unsafe {
                let cf_str = owner_value as core_foundation::string::CFStringRef;
                CFString::wrap_under_get_rule(cf_str).to_string()
            };

            if owner_str != app_name {
                continue;
            }

            // Check window layer (0 = normal window)
            let layer_key = CFString::new("kCGWindowLayer");
            let mut layer_value: *const std::ffi::c_void = std::ptr::null();
            let has_layer = unsafe {
                CFDictionaryGetValueIfPresent(
                    dict_ref,
                    layer_key.as_concrete_TypeRef() as *const std::ffi::c_void,
                    &mut layer_value,
                )
            };

            if has_layer != 0 && !layer_value.is_null() {
                let layer = unsafe {
                    let num = layer_value as core_foundation::number::CFNumberRef;
                    core_foundation::number::CFNumber::wrap_under_get_rule(num)
                        .to_i32()
                        .unwrap_or(999)
                };
                if layer != 0 {
                    continue;
                }
            }

            // Get window ID
            let id_key = CFString::new("kCGWindowNumber");
            let mut id_value: *const std::ffi::c_void = std::ptr::null();
            let has_id = unsafe {
                CFDictionaryGetValueIfPresent(
                    dict_ref,
                    id_key.as_concrete_TypeRef() as *const std::ffi::c_void,
                    &mut id_value,
                )
            };

            if has_id != 0 && !id_value.is_null() {
                let wid = unsafe {
                    let num = id_value as core_foundation::number::CFNumberRef;
                    core_foundation::number::CFNumber::wrap_under_get_rule(num)
                        .to_i32()
                        .unwrap_or(0) as u32
                };
                if wid != 0 {
                    return Ok(wid);
                }
            }
        }

        Err(PlatformError::CaptureError(format!(
            "No window found for app: {}",
            app_name
        )))
    }
}

impl PlatformProvider for MacOSProvider {
    fn capture_chat_area(&self) -> PlatformResult<Screenshot> {
        let path = self.temp_dir.join("screenshot.png");
        let app_name = self.frontmost_app_name().unwrap_or_else(|_| "Unknown".to_string());

        // Try to get frontmost window ID for targeted capture
        match self.frontmost_window_id() {
            Ok(window_id) => {
                // Use screencapture with -l flag to capture specific window
                let output = Command::new("screencapture")
                    .args(["-l", &window_id.to_string(), "-x", "-o", path.to_str().unwrap()])
                    .output()
                    .map_err(|e| PlatformError::CaptureError(format!("screencapture failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(PlatformError::CaptureError(format!(
                        "screencapture exited with error: {}",
                        stderr
                    )));
                }
            }
            Err(e) => {
                log::warn!("Could not get window ID ({}), falling back to full screen capture", e);
                // Fallback: capture the main screen
                let output = Command::new("screencapture")
                    .args(["-x", "-o", path.to_str().unwrap()])
                    .output()
                    .map_err(|e| PlatformError::CaptureError(format!("screencapture failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(PlatformError::CaptureError(format!(
                        "screencapture exited with error: {}",
                        stderr
                    )));
                }
            }
        }

        // Get image dimensions and crop to lower 2/3 (chat area)
        let metadata = std::fs::metadata(&path)
            .map_err(|e| PlatformError::CaptureError(format!("Screenshot file not found: {}", e)))?;

        if metadata.len() == 0 {
            return Err(PlatformError::CaptureError("Screenshot file is empty".into()));
        }

        // Use sips to get height and crop
        self.crop_to_chat_area(&path, &app_name)?;

        Ok(Screenshot {
            path,
            width: 0,  // We don't strictly need dimensions for OCR
            height: 0,
        })
    }

    fn ocr(&self, screenshot: &Screenshot) -> PlatformResult<String> {
        // Use our Swift OCR helper binary which supports Chinese
        let ocr_binary = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("compleo-ocr"))
            .unwrap_or_else(|| PathBuf::from("compleo-ocr"));

        // Fallback: look in the swift-ocr directory (dev mode)
        let ocr_binary = if ocr_binary.exists() {
            ocr_binary
        } else {
            // Try relative to the source tree (dev mode)
            let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("swift-ocr/compleo-ocr");
            if dev_path.exists() {
                dev_path
            } else {
                return Err(PlatformError::OcrError(
                    "compleo-ocr binary not found. Run: swiftc -O -o swift-ocr/compleo-ocr swift-ocr/main.swift -framework Vision -framework AppKit -framework CoreGraphics -framework ImageIO".into()
                ));
            }
        };

        let output = Command::new(&ocr_binary)
            .arg(screenshot.path.to_str().unwrap_or(""))
            .output()
            .map_err(|e| PlatformError::OcrError(format!("Failed to run OCR: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PlatformError::OcrError(format!("OCR process failed: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON output
        let json: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| PlatformError::OcrError(format!("Failed to parse OCR output: {}", e)))?;

        if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
            return Err(PlatformError::OcrError(error.to_string()));
        }

        let text = json
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if text.is_empty() {
            return Err(PlatformError::OcrError("No text recognized".into()));
        }

        Ok(text)
    }

    fn frontmost_app_name(&self) -> PlatformResult<String> {
        use objc2_app_kit::NSWorkspace;

        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace
            .frontmostApplication()
            .ok_or(PlatformError::NoFrontmostApp)?;
        let name = app
            .localizedName()
            .ok_or(PlatformError::NoFrontmostApp)?;
        Ok(name.to_string())
    }

    fn read_input_field(&self) -> PlatformResult<Option<String>> {
        // TODO: Implement Accessibility API reading
        // For now, return None (triggers Reply mode as designed)
        Ok(None)
    }

    fn set_clipboard(&self, text: &str) -> PlatformResult<()> {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::NSString;

        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let ns_string = NSString::from_str(text);
            let result = pasteboard.setString_forType(
                &ns_string,
                objc2_app_kit::NSPasteboardTypeString,
            );
            if !result {
                return Err(PlatformError::ClipboardError("Failed to set clipboard".into()));
            }
        }

        Ok(())
    }
}


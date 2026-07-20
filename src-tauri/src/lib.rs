use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio::sync::mpsc;

mod config;
mod distill;
mod llm;
mod platform;
mod storage;
use platform::PlatformProvider;
use std::sync::OnceLock;

/// Global storage instance
static STORAGE: OnceLock<storage::Storage> = OnceLock::new();

fn get_storage() -> &'static storage::Storage {
    STORAGE.get_or_init(|| {
        storage::Storage::open().unwrap_or_else(|e| {
            log::error!("Failed to open storage: {}", e);
            panic!("Storage initialization failed");
        })
    })
}

/// Get LLM config for embedding API calls (reuses main config)
fn llm_config_for_embed() -> config::Config {
    config::Config::load()
}

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindowStyleMask;

/// Global flag to prevent concurrent triggers
static BUSY: AtomicBool = AtomicBool::new(false);

/// Make a Tauri window non-activating on macOS (it won't steal focus).
#[cfg(target_os = "macos")]
fn make_window_non_activating(window: &tauri::WebviewWindow) {
    if let Ok(ns_window_ptr) = window.ns_window() {
        let ns_window = ns_window_ptr as *mut objc2_app_kit::NSWindow;
        unsafe {
            let ns_window = &*ns_window;
            let mut style = ns_window.styleMask();
            style.insert(NSWindowStyleMask::NonactivatingPanel);
            ns_window.setStyleMask(style);
            ns_window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
            ns_window.setHidesOnDeactivate(false);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tokio runtime for async LLM calls
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![config::get_config, config::save_config, open_main_window_cmd, quit_app])
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let cmd_period = Shortcut::new(Some(Modifiers::SUPER), Code::Period);

                        if shortcut == &cmd_period {
                            log::info!("Cmd+. triggered");
                            handle_trigger(app, &rt);
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            // Register only Cmd+. globally. Esc is registered/unregistered dynamically
            // when overlay or popover is visible to avoid blocking Esc in other apps.
            let cmd_period = Shortcut::new(Some(Modifiers::SUPER), Code::Period);
            app.global_shortcut().register(cmd_period)?;
            log::info!("Registered global shortcut: Cmd+.");

            // Create tray
            let open_app = MenuItem::with_id(app, "open_app", "Open Compleo", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_app, &quit])?;

            let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))
                .expect("failed to load tray icon");

            TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open_app" => {
                        open_main_window(app);
                    }
                    "quit" => {
                        log::info!("Quit requested");
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { button, button_state, .. } = event {
                        if button == tauri::tray::MouseButton::Left
                            && button_state == tauri::tray::MouseButtonState::Up
                        {
                            let app = tray.app_handle();
                            // Get tray icon position to place popover near it
                            let tray_rect = tray.rect().ok().flatten();
                            toggle_popover(app, tray_rect);
                        }
                    }
                })
                .tooltip("Compleo - AI Reply Assistant")
                .build(app)?;

            // Check LLM config on startup
            let cfg = config::Config::load();
            if cfg.api_key.is_empty() {
                log::warn!("API key not configured. Open Settings to configure.");
            } else {
                log::info!("LLM config loaded: model={}, base_url={}", cfg.model, cfg.base_url);
            }

            // Initialize storage
            let storage = get_storage();
            log::info!("Storage ready ({} records, {} undistilled)", storage.count(), storage.undistilled_count());

            // Start background distillation loop
            std::thread::spawn(|| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    // Wait a bit after startup before first run
                    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                    loop {
                        let config = config::Config::load();
                        if !config.api_key.is_empty() {
                            let storage = get_storage();
                            // Distill conversations
                            let distilled = distill::run_distillation(storage, &config).await;
                            // Generate embeddings
                            if distilled > 0 {
                                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            }
                            distill::run_embedding_generation(storage, &config).await;
                        }
                        // Run every 5 minutes or when there are undistilled items
                        tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                    }
                });
            });

            log::info!("Compleo started - tray icon active");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Compleo");
}

fn handle_trigger(app: &tauri::AppHandle, rt: &tokio::runtime::Runtime) {
    // Prevent concurrent triggers
    if BUSY.swap(true, Ordering::SeqCst) {
        log::info!("Busy, ignoring trigger");
        return;
    }

    // If overlay is visible, hide it first
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            let _ = app.emit("hide-recommendation", ());
            BUSY.store(false, Ordering::SeqCst);
            log::info!("Overlay hidden (re-trigger)");
            return;
        }
    }

    // Step 1: Screenshot + OCR (synchronous, fast)
    let (app_name, ocr_text) = capture_and_ocr();
    if ocr_text.starts_with('❌') {
        // Error - show in overlay
        ensure_overlay_window(app);
        let _ = app.emit("show-recommendation", &ocr_text);
        start_auto_hide(app.clone(), 5);
        BUSY.store(false, Ordering::SeqCst);
        return;
    }

    // Step 2: Check LLM config
    let app_config = config::Config::load();
    let llm_config = if app_config.api_key.is_empty() {
        let msg = "⚠️ 请先配置 API Key（点击菜单栏图标 → Settings）";
        ensure_overlay_window(app);
        let _ = app.emit("show-recommendation", msg);
        start_auto_hide(app.clone(), 5);
        BUSY.store(false, Ordering::SeqCst);
        return;
    } else {
        llm::LlmConfig {
            api_key: app_config.api_key,
            model: app_config.model,
            base_url: app_config.base_url,
        }
    };

    // Step 3: Show overlay with "thinking" state, then stream LLM response
    ensure_overlay_window(app);
    let _ = app.emit("stream-start", ());

    let app_handle = app.clone();

    // Spawn async LLM call
    rt.spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        // Get recent accepted replies for style matching
        let recent_replies = get_storage().recent_accepted_replies(&app_name, 3);
        let style_context = if recent_replies.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n[用户之前在该应用的回复风格参考]\n{}",
                recent_replies.iter()
                    .enumerate()
                    .map(|(i, r)| format!("{}. {}", i + 1, r))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        // Semantic search: find relevant historical messages
        let semantic_context = match distill::embed_query(&llm_config_for_embed(), &ocr_text).await {
            Ok(query_vec) => {
                let results = get_storage().semantic_search(&query_vec, &app_name, 3);
                if results.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\n[相关历史对话]\n{}",
                        results.iter()
                            .map(|(content, _score)| format!("- {}", content))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
            }
            Err(e) => {
                log::debug!("Semantic search skipped: {}", e);
                String::new()
            }
        };

        // Include app name + style + semantic context
        let context_with_app = format!(
            "[当前应用: {}]{}{}\n\n{}",
            app_name, style_context, semantic_context, ocr_text
        );

        let request = llm::LlmRequest {
            system_prompt: String::new(),
            current_context: context_with_app,
            draft: None,
            mode: llm::Mode::Reply,
        };

        // Spawn the streaming task
        let config_clone = llm_config.clone();
        let stream_task = tokio::spawn(async move {
            llm::generate_stream(&config_clone, request, tx).await
        });

        // Forward chunks to UI
        while let Some(chunk) = rx.recv().await {
            let _ = app_handle.emit("stream-chunk", &chunk);
        }

        // Get final result
        match stream_task.await {
            Ok(Ok(full_text)) => {
                log::info!("LLM response complete ({} chars)", full_text.len());

                // Save to storage
                let conv_id = get_storage()
                    .save_conversation(&app_name, &ocr_text, &full_text)
                    .unwrap_or_else(|e| { log::error!("Storage save failed: {}", e); -1 });

                // Mark as accepted (user will paste it)
                if conv_id > 0 {
                    let _ = get_storage().mark_accepted(conv_id);
                }

                // Copy to clipboard
                let provider = platform::MacOSProvider::new();
                if let Err(e) = provider.set_clipboard(&full_text) {
                    log::error!("Clipboard failed: {}", e);
                }
                let _ = app_handle.emit("stream-done", &full_text);
                start_auto_hide(app_handle, 8);
            }
            Ok(Err(e)) => {
                log::error!("LLM error: {}", e);
                // Save failed attempt (empty reply, not accepted)
                let _ = get_storage().save_conversation(&app_name, &ocr_text, "");
                let _ = app_handle.emit("stream-error", &e);
                start_auto_hide(app_handle, 5);
            }
            Err(e) => {
                log::error!("LLM task panicked: {}", e);
                let msg = format!("Internal error: {}", e);
                let _ = app_handle.emit("stream-error", &msg);
                start_auto_hide(app_handle, 5);
            }
        }

        BUSY.store(false, Ordering::SeqCst);
    });
}

/// Capture screenshot of frontmost window and run OCR
/// Returns (app_name, ocr_text) or error message prefixed with ❌
fn capture_and_ocr() -> (String, String) {
    let provider = platform::MacOSProvider::new();

    let app_name = match provider.frontmost_app_name() {
        Ok(name) => {
            log::info!("Frontmost app: {}", name);
            name
        }
        Err(e) => {
            log::warn!("Could not get app name: {}", e);
            "Unknown".to_string()
        }
    };

    let screenshot = match provider.capture_chat_area() {
        Ok(s) => {
            log::info!("Screenshot captured: {:?}", s.path);
            s
        }
        Err(e) => {
            log::error!("Screenshot failed: {}", e);
            return (app_name, format!("❌ Screenshot failed: {}", e));
        }
    };

    match provider.ocr(&screenshot) {
        Ok(text) => {
            log::info!("OCR result ({} chars)", text.len());
            (app_name, text)
        }
        Err(e) => {
            log::error!("OCR failed: {}", e);
            (app_name, format!("❌ OCR failed: {}", e))
        }
    }
}

fn ensure_overlay_window(app: &tauri::AppHandle) {
    if app.get_webview_window("overlay").is_some() {
        // Already exists, just show it
        if let Some(w) = app.get_webview_window("overlay") {
            let _ = w.show();
        }
        return;
    }

    // Create overlay window
    let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
        .title("Compleo Overlay")
        .inner_size(420.0, 200.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .transparent(true)
        .resizable(false)
        .shadow(false)
        .visible_on_all_workspaces(true)
        .center()
        .build();

    match window {
        Ok(w) => {
            #[cfg(target_os = "macos")]
            make_window_non_activating(&w);
            log::info!("Overlay window created");
        }
        Err(e) => log::error!("Failed to create overlay: {}", e),
    }
}

fn hide_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
        let _ = app.emit("hide-recommendation", ());
    }
    BUSY.store(false, Ordering::SeqCst);
}

fn start_auto_hide(app: tauri::AppHandle, seconds: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        if let Some(w) = app.get_webview_window("overlay") {
            if w.is_visible().unwrap_or(false) {
                let _ = w.hide();
                let _ = app.emit("hide-recommendation", ());
                log::info!("Overlay auto-hidden after {}s timeout", seconds);
            }
        }
    });
}

fn open_settings_window(app: &tauri::AppHandle) {
    open_main_window(app);
}

fn toggle_popover(app: &tauri::AppHandle, tray_rect: Option<tauri::Rect>) {
    if let Some(window) = app.get_webview_window("popover") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            // Reposition near tray icon (aligned to icon's right edge)
            if let Some(rect) = &tray_rect {
                let pos = rect.position.to_physical::<f64>(1.0);
                let size = rect.size.to_physical::<f64>(1.0);
                // Align popover's right edge with tray icon's right edge
                let popover_width = 260.0;
                let x = pos.x + size.width - popover_width;
                let y = pos.y + size.height + 4.0;
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x as i32, y as i32)));
            }
            let _ = window.show();
            let _ = window.set_focus();
        }
        return;
    }

    // Create popover window
    let popover_width = 260.0;
    let popover_height = 180.0;

    let mut builder = WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("popover.html".into()))
        .title("")
        .inner_size(popover_width, popover_height)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .transparent(true)
        .shadow(true);

    if let Some(rect) = tray_rect {
        let pos = rect.position.to_physical::<f64>(1.0);
        let size = rect.size.to_physical::<f64>(1.0);
        let x = pos.x + size.width - popover_width;
        let y = pos.y + size.height + 4.0;
        builder = builder.position(x, y);
    } else {
        builder = builder.center();
    }

    match builder.build() {
        Ok(window) => {
            // Apply macOS vibrancy (dark material)
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                let _ = apply_vibrancy(&window, NSVisualEffectMaterial::Popover, None, Some(12.0));
            }
            log::info!("Popover opened");
        }
        Err(e) => log::error!("Failed to open popover: {}", e),
    }
}

fn open_main_window(app: &tauri::AppHandle) {
    // Hide popover if visible
    if let Some(popover) = app.get_webview_window("popover") {
        let _ = popover.hide();
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    match WebviewWindowBuilder::new(app, "main", WebviewUrl::App("main.html".into()))
        .title("Compleo")
        .inner_size(640.0, 440.0)
        .min_inner_size(500.0, 380.0)
        .transparent(true)
        .center()
        .build()
    {
        Ok(window) => {
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                let _ = apply_vibrancy(&window, NSVisualEffectMaterial::Sidebar, None, None);
            }
            log::info!("Main window opened");
        }
        Err(e) => log::error!("Failed to open main window: {}", e),
    }
}

#[tauri::command]
fn open_main_window_cmd(app: tauri::AppHandle) {
    open_main_window(&app);
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

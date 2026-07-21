use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio::sync::mpsc;

mod auto_trigger;
mod config;
mod distill;
mod input_monitor;
mod llm;
mod platform;
mod storage;
use platform::PlatformProvider;
use std::sync::OnceLock;

/// Global storage instance
static STORAGE: OnceLock<storage::Storage> = OnceLock::new();

/// Global input monitor state
static INPUT_STATE: OnceLock<Arc<Mutex<input_monitor::InputMonitorState>>> = OnceLock::new();

use std::sync::Arc;

fn get_storage() -> &'static storage::Storage {
    STORAGE.get_or_init(|| {
        storage::Storage::open().unwrap_or_else(|e| {
            log::error!("Failed to open storage: {}", e);
            panic!("Storage initialization failed");
        })
    })
}

fn get_input_state() -> &'static Arc<Mutex<input_monitor::InputMonitorState>> {
    INPUT_STATE.get_or_init(|| Arc::new(Mutex::new(input_monitor::InputMonitorState::new())))
}

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindowStyleMask;

/// Global flag to prevent concurrent triggers
static BUSY: AtomicBool = AtomicBool::new(false);

/// Cached tray icon rectangle for positioning the overlay bubble
static TRAY_RECT: OnceLock<Mutex<Option<tauri::Rect>>> = OnceLock::new();

fn get_tray_rect() -> Option<tauri::Rect> {
    TRAY_RECT.get_or_init(|| Mutex::new(None)).lock().ok()?.clone()
}

fn set_tray_rect(rect: tauri::Rect) {
    let store = TRAY_RECT.get_or_init(|| Mutex::new(None));
    if let Ok(mut r) = store.lock() {
        *r = Some(rect);
    }
}

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
            // Hide from Dock by default, only show in menu bar
            #[cfg(target_os = "macos")]
            app.handle().set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            let cmd_period = Shortcut::new(Some(Modifiers::SUPER), Code::Period);
            app.global_shortcut().register(cmd_period)?;
            log::info!("Registered global shortcut: Cmd+.");

            // Create tray
            let open_app = MenuItem::with_id(app, "open_app", "设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_app, &quit])?;

            let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))
                .expect("failed to load tray icon");

            TrayIconBuilder::with_id("main-tray")
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
                        // Cache tray position for overlay bubble
                        if let Some(rect) = tray.rect().ok().flatten() {
                            set_tray_rect(rect);
                        }
                        if button == tauri::tray::MouseButton::Left
                            && button_state == tauri::tray::MouseButtonState::Up
                        {
                            // Left click: open main app window
                            open_main_window(tray.app_handle());
                        }
                        // Right click: system shows the menu automatically
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

            // Pre-create overlay window (hidden) so it's ready on first Cmd+. trigger
            {
                let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
                    .title("Compleo Overlay")
                    .inner_size(340.0, 160.0)
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .focused(false)
                    .transparent(true)
                    .resizable(false)
                    .shadow(true)
                    .visible_on_all_workspaces(true)
                    .visible(false)
                    .center()
                    .build();

                match window {
                    Ok(w) => {
                        #[cfg(target_os = "macos")]
                        make_window_non_activating(&w);
                        log::info!("Overlay window pre-created (hidden)");
                    }
                    Err(e) => log::error!("Failed to pre-create overlay: {}", e),
                }
            }

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
                            // Distill style profiles
                            if distilled > 0 {
                                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            }
                            distill::distill_style_profiles(storage, &config).await;
                        }
                        // Run every 5 minutes
                        tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                    }
                });
            });

            // Start app-switch monitoring + input monitor

            // Thread 1: CGEventTap for keystroke monitoring
            let input_state = get_input_state().clone();
            std::thread::spawn(move || {
                input_monitor::start_event_tap(input_state);
            });

            // Thread 2: App-switch polling + Enter learning
            std::thread::spawn(move || {
                use objc2_app_kit::NSWorkspace;

                let monitor_config = input_monitor::InputMonitorConfig::default();
                let mut last_app = String::new();

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));

                    // Update current app in input monitor state
                    let workspace = NSWorkspace::sharedWorkspace();
                    let current_app = workspace
                        .frontmostApplication()
                        .and_then(|a| a.localizedName())
                        .map(|n| n.to_string())
                        .unwrap_or_default();

                    if current_app != last_app && !current_app.is_empty() {
                        log::info!("App switch: {} → {}", last_app, current_app);
                        last_app = current_app.clone();
                    }

                    // Update input state with current app
                    if let Ok(mut state) = get_input_state().lock() {
                        state.current_app = current_app.clone();
                    }

                    // Only process chat apps
                    if !auto_trigger::is_chat_app(&current_app) {
                        continue;
                    }

                    let mut input_state = match get_input_state().lock() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

                    // Enter detected → learning capture
                    if input_state.take_enter() {
                        let app_name = current_app.clone();
                        drop(input_state);

                        // Wait for message to render in chat
                        std::thread::sleep(std::time::Duration::from_millis(
                            monitor_config.enter_capture_delay_ms
                        ));

                        // Capture and extract the last user message
                        let provider = platform::MacOSProvider::new();
                        if let Ok(screenshot) = provider.capture_chat_area() {
                            if let Ok(ocr_text) = provider.ocr(&screenshot) {
                                let last_user_msg: Option<&str> = ocr_text.lines()
                                    .filter(|l| l.starts_with("→ "))
                                    .last()
                                    .map(|l| l.trim_start_matches("→ ").trim());

                                if let Some(msg) = last_user_msg {
                                    if msg.len() > 1 {
                                        let _ = get_storage().save_conversation(&app_name, &ocr_text, msg);
                                        let _ = get_storage().mark_accepted(get_storage().count());
                                        log::info!("Enter learning: captured '{}' from {}", &msg[..msg.len().min(30)], app_name);
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // Reset typing state if idle for 10s
                    if input_state.is_typing && input_state.last_keystroke.elapsed() > std::time::Duration::from_secs(10) {
                        input_state.is_typing = false;
                    }

                    drop(input_state);
                }
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
        ensure_overlay_window(app);
        let _ = app.emit("show-recommendation", &ocr_text);
        start_auto_hide(app.clone(), 3);
        BUSY.store(false, Ordering::SeqCst);
        return;
    }

    // Step 2: Check LLM config
    let app_config = config::Config::load();
    let llm_config = if app_config.api_key.is_empty() {
        let msg = "⚠️ 请先配置 API Key（点击菜单栏图标 → Settings）";
        ensure_overlay_window(app);
        let _ = app.emit("show-recommendation", msg);
        start_auto_hide(app.clone(), 3);
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

        // Get distilled style profile (more powerful than raw examples)
        let profile_context = get_storage()
            .get_style_profile(&app_name)
            .map(|p| format!("\n\n[用户说话风格画像]\n{}", p))
            .unwrap_or_default();

        // Include app name + style context
        let context_with_app = format!(
            "[当前应用: {}]{}{}\n\n{}",
            app_name, profile_context, style_context, ocr_text
        );

        let request = llm::LlmRequest {
            current_context: context_with_app,
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
                start_auto_hide(app_handle, 5);
            }
            Ok(Err(e)) => {
                log::error!("LLM error: {}", e);
                // Save failed attempt (empty reply, not accepted)
                let _ = get_storage().save_conversation(&app_name, &ocr_text, "");
                let _ = app_handle.emit("stream-error", &e);
                start_auto_hide(app_handle, 3);
            }
            Err(e) => {
                log::error!("LLM task panicked: {}", e);
                let msg = format!("Internal error: {}", e);
                let _ = app_handle.emit("stream-error", &msg);
                start_auto_hide(app_handle, 3);
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
    if let Some(w) = app.get_webview_window("overlay") {
        // Position near tray icon
        position_overlay_near_tray(&w);
        let _ = w.show();
        return;
    }

    // Fallback: create if somehow missing (should not happen after pre-creation)
    let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
        .title("Compleo Overlay")
        .inner_size(340.0, 160.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .transparent(true)
        .resizable(false)
        .shadow(true)
        .visible_on_all_workspaces(true)
        .center()
        .build();

    match window {
        Ok(w) => {
            #[cfg(target_os = "macos")]
            make_window_non_activating(&w);
            position_overlay_near_tray(&w);
            log::info!("Overlay window created (fallback)");
        }
        Err(e) => log::error!("Failed to create overlay: {}", e),
    }
}

/// Position overlay window below the tray icon (right-aligned)
fn position_overlay_near_tray(window: &tauri::WebviewWindow) {
    // Try cached rect first, then query tray icon directly
    let rect = get_tray_rect().or_else(|| {
        let app = window.app_handle();
        app.tray_by_id("main-tray")?.rect().ok().flatten()
    });

    if let Some(rect) = rect {
        let pos = rect.position.to_physical::<f64>(1.0);
        let size = rect.size.to_physical::<f64>(1.0);
        let overlay_width = 340.0;
        // Right-align with tray icon
        let x = pos.x + size.width - overlay_width;
        let y = pos.y + size.height + 4.0;
        let _ = window.set_position(tauri::Position::Physical(
            tauri::PhysicalPosition::new(x as i32, y as i32),
        ));
    }
    // If no tray rect available, stays at last position (or centered)
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

    // Show in Dock when main window is open
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

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

            // Listen for close to hide from Dock
            let app_handle = app.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    #[cfg(target_os = "macos")]
                    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            });

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

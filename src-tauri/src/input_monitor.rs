//! Input monitor using CGEventTap.
//! Monitors keystrokes in configured chat apps to detect Enter key,
//! triggering learning capture of sent messages.

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};
use core_foundation::runloop::CFRunLoop;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::auto_trigger;

/// Configuration for input monitoring behavior.
#[derive(Clone)]
pub struct InputMonitorConfig {
    /// How long to wait after Enter before capturing (ms).
    pub enter_capture_delay_ms: u64,
}

impl Default for InputMonitorConfig {
    fn default() -> Self {
        Self {
            enter_capture_delay_ms: 100,
        }
    }
}

/// Shared state for the input monitor.
pub struct InputMonitorState {
    /// Whether we just detected an Enter key (consumed by learning loop).
    pub enter_detected: bool,
    /// Current frontmost app name (updated externally by polling thread).
    pub current_app: String,
    /// Time of last keystroke (for idle detection).
    pub last_keystroke: Instant,
    /// Whether typing is active.
    pub is_typing: bool,
}

impl InputMonitorState {
    pub fn new() -> Self {
        Self {
            enter_detected: false,
            current_app: String::new(),
            last_keystroke: Instant::now(),
            is_typing: false,
        }
    }

    /// Take the enter_detected flag (resets it).
    pub fn take_enter(&mut self) -> bool {
        let v = self.enter_detected;
        self.enter_detected = false;
        v
    }
}

/// Start the CGEventTap monitoring loop.
/// Blocks the calling thread (runs CFRunLoop). Call from a dedicated thread.
/// Retries creation up to 10 times with increasing delay for permission propagation.
pub fn start_event_tap(state: Arc<Mutex<InputMonitorState>>) {
    log::info!("Starting CGEventTap for input monitoring...");

    let max_retries = 10;
    let mut attempt = 0;

    loop {
        attempt += 1;

        let state_clone = state.clone();
        let result = CGEventTap::with_enabled(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![CGEventType::KeyDown],
            |_proxy, _event_type, event| {
                handle_key_event(event, &state_clone);
                CallbackResult::Keep
            },
            || {
                log::info!("CGEventTap active - monitoring keyboard input");
                CFRunLoop::run_current();
            },
        );

        match result {
            Ok(_) => {
                log::info!("CGEventTap loop ended");
                return;
            }
            Err(()) => {
                if attempt >= max_retries {
                    log::error!(
                        "CGEventTap failed after {} attempts. \
                        Enter-triggered learning unavailable (grant Accessibility permission).",
                        max_retries
                    );
                    return;
                }
                let delay_secs = (attempt * 3).min(15) as u64;
                log::warn!(
                    "CGEventTap failed (attempt {}/{}), retrying in {}s...",
                    attempt, max_retries, delay_secs
                );
                std::thread::sleep(Duration::from_secs(delay_secs));
            }
        }
    }
}

fn handle_key_event(event: &CGEvent, state: &Arc<Mutex<InputMonitorState>>) {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();

    let mut state = match state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Only process if current app is a chat app
    if !auto_trigger::is_chat_app(&state.current_app) {
        return;
    }

    // Ignore Cmd+key shortcuts
    if flags.contains(CGEventFlags::CGEventFlagCommand) {
        return;
    }

    // Keycode 36 = Return/Enter, 76 = Numpad Enter
    if keycode == 36 || keycode == 76 {
        state.enter_detected = true;
        state.is_typing = false;
        return;
    }

    // Skip function keys, arrows, modifiers (keycode >= 122)
    if keycode >= 122 {
        return;
    }

    // Regular typing key
    state.is_typing = true;
    state.last_keystroke = Instant::now();
}

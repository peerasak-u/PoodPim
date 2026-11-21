//! OS-specific pieces: the global push-to-talk key, the paste keystroke, permissions.
//!
//! Push-to-talk key: Right Option by default on macOS (a setting, see hotkey.rs), always Right
//! Alt on Windows. Esc cancels (macOS: or another key, a setting).

use crate::hotkey::{Binding, Chord, Reject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    TalkDown,
    TalkUp,
    /// macOS: another key went down right after the talk key (hotkey::TYPING_GUARD): the user
    /// is typing a combination (⌥+e), not dictating. Never sent on Windows.
    TalkAborted,
    /// The cancel key (Esc unless set otherwise).
    Escape,
    /// macOS, Settings recording a new talk key: this one was pressed (and is now in use).
    Captured(Binding),
    /// macOS, Settings recording a new cancel key: this one was pressed (and is now in use).
    CancelCaptured(Chord),
    /// Recording a new talk key: this key can't be used; still recording.
    CaptureRejected(Reject),
    /// Recording a new talk key: Esc, or it timed out.
    CaptureCancelled,
    /// The listener could not start (macOS: Accessibility / Input Monitoring not granted). It keeps retrying.
    PermissionMissing,
    /// The listener is running.
    Ready,
}

/// Texts for the server settings dialogs (shared by both platforms).
pub const URL_PROMPT: &str = "ใส่ API URL ของเซิร์ฟเวอร์ PoodPim (ขึ้นต้นด้วย https://)\n(ขอได้จากคนที่ส่งแอปนี้ให้คุณ)";
pub const KEY_PROMPT: &str = "ใส่ API key\n(ขอได้จากคนที่ส่งแอปนี้ให้คุณ)";
pub const KEY_KEEP_PROMPT: &str = "ใส่ API key\n(เว้นว่างไว้ = ใช้ key เดิม)";

/// How to start talking with this key and press mode, for the "already running" message.
pub fn how_to_talk(key: &str, mode: crate::hotkey::TalkMode) -> String {
    use crate::hotkey::TalkMode;
    match mode {
        TalkMode::Auto => format!("แตะ {key} เพื่อเริ่มพูด แอปจะหยุดเองเมื่อคุณเงียบ หรือกด {key} ค้างไว้ระหว่างพูดก็ได้"),
        TalkMode::Hold => format!("กด {key} ค้างไว้เพื่อพูดได้เลย"),
        TalkMode::Toggle => format!("แตะ {key} เพื่อเริ่มพูด แล้วแตะอีกครั้งเมื่อจบ"),
    }
}

/// The tray icon, as the "already running" message describes it.
const TRAY_ICON: &str = "ไอคอน PoodPim (กรอบคำพูดที่มีคลื่นเสียง)";

pub type HotkeySink = Box<dyn Fn(HotkeyEvent) + Send + Sync + 'static>;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("poodpim supports macOS and Windows");

//! OS-specific pieces: the global push-to-talk key, the paste keystroke, permissions.
//!
//! Push-to-talk key: Right Option (macOS) / Right Alt (Windows). Esc cancels.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    TalkDown,
    TalkUp,
    Escape,
    /// The listener could not start (macOS: Accessibility / Input Monitoring not granted). It keeps retrying.
    PermissionMissing,
    /// The listener is running.
    Ready,
}

/// Texts for the server settings dialogs (shared by both platforms).
pub const URL_PROMPT: &str = "ใส่ API URL ของเซิร์ฟเวอร์ PoodPim (ขึ้นต้นด้วย https://)\n(ขอได้จากคนที่ส่งแอปนี้ให้คุณ)";
pub const KEY_PROMPT: &str = "ใส่ API key\n(ขอได้จากคนที่ส่งแอปนี้ให้คุณ)";
pub const KEY_KEEP_PROMPT: &str = "ใส่ API key\n(เว้นว่างไว้ = ใช้ key เดิม)";

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

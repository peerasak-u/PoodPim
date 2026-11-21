//! The talk key: which key it is and how a press is read (both settings), and the matcher that
//! turns the macOS keyboard tap's key events into talk events.
//!
//! Windows keeps its fixed Right Alt hook (platform/windows.rs) and doesn't use the matcher.
//! The press mode is read in app.rs, so it works on both.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::platform::{self, HotkeyEvent};

/// Another key pressed this soon after the talk modifier went down means the user is typing a
/// combination (⌥+e, ⌃A), not dictating (macOS only: there the modifier still reaches apps).
const TYPING_GUARD: Duration = Duration::from_millis(500);

/// A modifier key that can be the talk key on its own. ⌘ and ⇧ can't: they are held for
/// shortcuts and capitals all the time, so every press would open the microphone (and switch
/// a Bluetooth headset to call mode), and a ⌘C after holding ⌘ a little would type a transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModKey {
    RightOption,
    RightControl,
    /// The fn / 🌐 key (macOS).
    Fn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    /// One modifier key on its own.
    Modifier { key: ModKey },
}

impl Binding {
    pub const DEFAULT: Binding = Binding::Modifier { key: ModKey::RightOption };

    /// The key in use: on Windows always Right Alt (its keyboard hook is fixed).
    pub fn usable(self) -> Binding {
        if cfg!(windows) { Binding::DEFAULT } else { self }
    }

    /// For the menu, the pill and Settings.
    pub fn label(&self) -> &'static str {
        match self {
            Binding::Modifier { key: ModKey::RightOption } => platform::TALK_KEY_LABEL,
            Binding::Modifier { key: ModKey::RightControl } => "Right ⌃",
            Binding::Modifier { key: ModKey::Fn } => "fn",
        }
    }
}

impl Default for Binding {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What a press of the talk key does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkMode {
    /// Hold to talk, let go to finish; a quick tap starts hands-free, the next tap finishes.
    #[default]
    HoldOrTap,
    /// Hold to talk, let go to finish, however short the press.
    Hold,
    /// Every press starts hands-free; the next press finishes.
    Toggle,
}

/// A key event from the keyboard tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawKey {
    /// A modifier key that can be the talk key went down or up.
    Mod { key: ModKey, down: bool },
    /// Any other key went down (not Esc); `repeat`: the OS repeating a key held down.
    Down { repeat: bool },
}

pub struct Matcher {
    binding: Binding,
    /// The talk key is down, since then.
    held: Option<Instant>,
    /// TalkAborted was already sent for this press.
    aborted: bool,
}

impl Matcher {
    pub const fn new(binding: Binding) -> Self {
        Self { binding, held: None, aborted: false }
    }

    pub fn set_binding(&mut self, b: Binding) {
        self.binding = b;
        self.held = None;
    }

    pub fn feed(&mut self, k: RawKey, at: Instant) -> Option<HotkeyEvent> {
        let Binding::Modifier { key: bound } = self.binding;
        match k {
            // macOS never repeats a modifier, so a second "down" means the release was missed
            // (the tap was off for a moment, or a password field hid it): a new press.
            RawKey::Mod { key, down: true } if key == bound => {
                self.held = Some(at);
                self.aborted = false;
                Some(HotkeyEvent::TalkDown)
            }
            RawKey::Mod { key, down: false } if key == bound && self.held.is_some() => {
                self.held = None;
                Some(HotkeyEvent::TalkUp)
            }
            RawKey::Mod { .. } => None,
            RawKey::Down { repeat: false } => match self.held {
                Some(t) if !self.aborted && at.saturating_duration_since(t) < TYPING_GUARD => {
                    self.aborted = true;
                    Some(HotkeyEvent::TalkAborted)
                }
                _ => None,
            },
            RawKey::Down { repeat: true } => None,
        }
    }
}

/// The matcher the macOS keyboard tap feeds (the app sets its key from the config).
pub fn lock() -> MutexGuard<'static, Matcher> {
    static MATCHER: Mutex<Matcher> = Mutex::new(Matcher::new(Binding::DEFAULT));
    MATCHER.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn opt(down: bool) -> RawKey {
        RawKey::Mod { key: ModKey::RightOption, down }
    }

    fn ctrl(down: bool) -> RawKey {
        RawKey::Mod { key: ModKey::RightControl, down }
    }

    const KEY: RawKey = RawKey::Down { repeat: false };

    /// Feeds `k` to `m` at `t0 + ms`.
    fn at(m: &mut Matcher, t0: Instant, ms: u64, k: RawKey) -> Option<HotkeyEvent> {
        m.feed(k, t0 + Duration::from_millis(ms))
    }

    #[test]
    fn serde_round_trips() {
        for key in [ModKey::RightOption, ModKey::RightControl, ModKey::Fn] {
            let b = Binding::Modifier { key };
            let s = serde_json::to_string(&b).unwrap();
            assert_eq!(serde_json::from_str::<Binding>(&s).unwrap(), b, "{s}");
        }
        assert_eq!(serde_json::to_value(Binding::DEFAULT).unwrap(), serde_json::json!({"type": "modifier", "key": "right_option"}));
        for m in [TalkMode::HoldOrTap, TalkMode::Hold, TalkMode::Toggle] {
            let s = serde_json::to_string(&m).unwrap();
            assert_eq!(serde_json::from_str::<TalkMode>(&s).unwrap(), m);
        }
        assert_eq!(serde_json::to_string(&TalkMode::HoldOrTap).unwrap(), "\"hold_or_tap\"");
        // ⌘ and ⇧ are not talk keys.
        assert!(serde_json::from_str::<Binding>(r#"{"type":"modifier","key":"right_command"}"#).is_err());
        assert!(serde_json::from_str::<Binding>(r#"{"type":"modifier","key":"left_shift"}"#).is_err());
    }

    #[test]
    fn old_config_keeps_right_option_hold_or_tap() {
        let c: Config = serde_json::from_str(r#"{"mixed_english":false,"live_typing":true}"#).unwrap();
        assert_eq!(c.talk_key, Binding::DEFAULT);
        assert_eq!(c.talk_mode, TalkMode::HoldOrTap);
        assert!(!c.mixed_english);
        let c: Config = serde_json::from_str(r#"{"talk_key":{"type":"modifier","key":"fn"},"talk_mode":"toggle"}"#).unwrap();
        assert_eq!(c.talk_key, Binding::Modifier { key: ModKey::Fn });
        assert_eq!(c.talk_mode, TalkMode::Toggle);
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!((back.talk_key, back.talk_mode), (c.talk_key, c.talk_mode));
    }

    #[test]
    fn windows_always_uses_right_alt() {
        let fn_key = Binding::Modifier { key: ModKey::Fn };
        assert_eq!(fn_key.usable(), if cfg!(windows) { Binding::DEFAULT } else { fn_key });
        assert_eq!(Binding::DEFAULT.label(), platform::TALK_KEY_LABEL);
    }

    #[test]
    fn modifier_down_up() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        assert_eq!(at(&mut m, t, 0, opt(true)), Some(HotkeyEvent::TalkDown));
        assert_eq!(at(&mut m, t, 900, opt(false)), Some(HotkeyEvent::TalkUp));
        assert_eq!(at(&mut m, t, 950, opt(false)), None);
        // Another key than the bound one.
        assert_eq!(at(&mut m, t, 1000, ctrl(true)), None);
        assert_eq!(at(&mut m, t, 1100, ctrl(false)), None);
        m.set_binding(Binding::Modifier { key: ModKey::RightControl });
        assert_eq!(at(&mut m, t, 1200, ctrl(true)), Some(HotkeyEvent::TalkDown));
        assert_eq!(at(&mut m, t, 1300, opt(true)), None);
    }

    #[test]
    fn missed_release_does_not_eat_the_next_press() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        assert_eq!(at(&mut m, t, 0, opt(true)), Some(HotkeyEvent::TalkDown));
        // The release never arrived (tap off for a moment, password field): the next press starts.
        assert_eq!(at(&mut m, t, 5000, opt(true)), Some(HotkeyEvent::TalkDown));
        assert_eq!(at(&mut m, t, 6000, opt(false)), Some(HotkeyEvent::TalkUp));
        // Key changed while held: its release is not a talk event, and the next press works.
        assert_eq!(at(&mut m, t, 7000, opt(true)), Some(HotkeyEvent::TalkDown));
        m.set_binding(Binding::DEFAULT);
        assert_eq!(at(&mut m, t, 7500, opt(false)), None);
        assert_eq!(at(&mut m, t, 8000, opt(true)), Some(HotkeyEvent::TalkDown));
    }

    #[test]
    fn typing_guard_judges_each_press() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        // ⌥+e typed quickly: one TalkAborted, and the release still comes.
        at(&mut m, t, 0, opt(true));
        assert_eq!(at(&mut m, t, 120, KEY), Some(HotkeyEvent::TalkAborted));
        assert_eq!(at(&mut m, t, 140, KEY), None);
        assert_eq!(at(&mut m, t, 200, opt(false)), Some(HotkeyEvent::TalkUp));
        // A key pressed later in a long hold is not typing a combination.
        at(&mut m, t, 1000, opt(true));
        assert_eq!(at(&mut m, t, 1600, KEY), None);
        at(&mut m, t, 2000, opt(false));
        // Repeats of a key held from before don't count; nor keys while the talk key is up.
        at(&mut m, t, 3000, opt(true));
        assert_eq!(at(&mut m, t, 3050, RawKey::Down { repeat: true }), None);
        at(&mut m, t, 3600, opt(false));
        assert_eq!(at(&mut m, t, 3650, KEY), None);
        // Each press is timed from its own start: a press right after a long one still counts.
        at(&mut m, t, 4000, opt(true));
        at(&mut m, t, 4350, opt(false));
        at(&mut m, t, 4450, opt(true));
        assert_eq!(at(&mut m, t, 4500, KEY), Some(HotkeyEvent::TalkAborted));
        // A missed release: the new press is timed from itself, not from the stale one.
        at(&mut m, t, 6000, opt(true));
        assert_eq!(at(&mut m, t, 6100, KEY), Some(HotkeyEvent::TalkAborted));
        // After a key change while held, keys don't count until the new key goes down.
        m.set_binding(Binding::DEFAULT);
        assert_eq!(at(&mut m, t, 6200, KEY), None);
    }
}

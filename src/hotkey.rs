//! The talk key: which key it is (a setting), how a press is read (press mode), and the state
//! machine that turns raw key events from the OS keyboard hook into talk events.
//!
//! No OS calls here: the platform listeners (platform/macos.rs, platform/windows.rs) feed
//! every key event to `lock().feed(…)` and act on the verdict (send the event to the app,
//! swallow the key or let it through).

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize};

use crate::platform::{self, HotkeyEvent};

/// Hold-or-tap mode: let go sooner than this = a tap, which starts hands-free recording.
pub const TAP: Duration = Duration::from_millis(300);
/// With a modifier as the talk key, another key pressed this soon after it went down means the
/// user is typing a combination (⌥+e), not dictating: the recording is dropped.
const TYPING_GUARD: Duration = Duration::from_millis(500);

/// One physical modifier key. Option = Alt and Command = Win on Windows; Fn is macOS only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModKey {
    LeftControl,
    RightControl,
    LeftOption,
    RightOption,
    LeftShift,
    RightShift,
    LeftCommand,
    RightCommand,
    Fn,
}

impl ModKey {
    fn bit(self) -> u16 {
        1 << self as u16
    }

    /// The id the Settings page uses (same as in config.json).
    pub fn id(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    pub fn from_id(id: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(id.to_string())).ok()
    }
}

/// Modifiers held with a chord's key, either side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

impl Mods {
    /// Shift alone doesn't make a key safe to take over: ⇧+letter is typing.
    fn has_trigger(&self) -> bool {
        self.control || self.option || self.command
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    /// One modifier key on its own (left and right are different keys).
    Modifier { key: ModKey },
    /// A key with modifiers, matched exactly. `code`: macOS virtual keycode / Windows VK code.
    Chord { code: u16, mods: Mods },
}

impl Binding {
    pub const DEFAULT: Binding = Binding::Modifier { key: ModKey::RightOption };

    pub fn validate(&self, class: KeyClass) -> Result<(), Reject> {
        match *self {
            Binding::Modifier { key: ModKey::Fn } if cfg!(windows) => Err(Reject::Unsupported),
            Binding::Modifier { .. } => Ok(()),
            Binding::Chord { .. } if class == KeyClass::Escape => Err(Reject::Escape),
            Binding::Chord { mods, .. } if !mods.has_trigger() && class != KeyClass::Function => Err(Reject::NeedsModifier),
            Binding::Chord { .. } => Ok(()),
        }
    }

    /// This binding, or the default if it can't be used here (an edited or copied config.json).
    pub fn usable(self) -> Binding {
        let class = match self {
            Binding::Chord { code, .. } => platform::key_class(code),
            Binding::Modifier { .. } => KeyClass::Other,
        };
        if self.validate(class).is_ok() { self } else { Binding::DEFAULT }
    }

    pub fn label(&self) -> String {
        match *self {
            Binding::Modifier { key } => platform::mod_label(key).to_string(),
            Binding::Chord { code, mods } => {
                let names: &[(bool, &str)] = if cfg!(target_os = "macos") {
                    &[(mods.control, "⌃"), (mods.option, "⌥"), (mods.shift, "⇧"), (mods.command, "⌘")]
                } else {
                    &[(mods.control, "Ctrl+"), (mods.option, "Alt+"), (mods.shift, "Shift+"), (mods.command, "Win+")]
                };
                let mut s: String = names.iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect();
                s.push_str(&platform::key_name(code));
                s
            }
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

/// A key as far as binding it goes (from `platform::key_class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyClass {
    Escape,
    /// F1–F20 and other keys that never type a character: fine on their own.
    Function,
    Other,
}

/// Why a key can't be the talk key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Esc cancels a recording.
    Escape,
    /// A typing key needs ⌃, ⌥ or ⌘ with it.
    NeedsModifier,
    /// Not available on this OS (fn on Windows).
    Unsupported,
}

/// A config.json value this build can't read falls back to the default instead of making the
/// whole file fail (which would reset every setting).
pub fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(v).unwrap_or_default())
}

/// Hands-free from the first press (Toggle), instead of only after a tap.
pub fn starts_handsfree(mode: TalkMode) -> bool {
    mode == TalkMode::Toggle
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// It was a tap: keep recording hands-free.
    Handsfree,
    /// Finish the recording.
    Stop,
    Ignore,
}

/// What letting go of the talk key does, after holding it for `held`.
pub fn on_release(mode: TalkMode, held: Duration) -> Release {
    match mode {
        TalkMode::HoldOrTap if held < TAP => Release::Handsfree,
        TalkMode::HoldOrTap | TalkMode::Hold => Release::Stop,
        TalkMode::Toggle => Release::Ignore,
    }
}

/// A key event as the OS hook saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawKey {
    Mod { key: ModKey, down: bool },
    Down { code: u16, mods: Mods, repeat: bool },
    Up { code: u16 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub event: Option<HotkeyEvent>,
    /// Keep the key from the focused app.
    pub swallow: bool,
}

impl Verdict {
    fn pass(event: Option<HotkeyEvent>) -> Self {
        Self { event, swallow: false }
    }

    fn swallow(event: Option<HotkeyEvent>) -> Self {
        Self { event, swallow: true }
    }
}

/// Recording a new talk key from Settings.
#[derive(Clone, Copy, Debug, Default)]
struct Capture {
    /// Modifiers down now (ModKey::bit).
    down: u16,
    /// The modifier that went down first, while nothing else has been pressed since.
    lone: Option<ModKey>,
}

pub struct Matcher {
    binding: Binding,
    /// The talk key is down, since then.
    held: Option<Instant>,
    /// A key typed with the talk modifier already dropped this press (see TYPING_GUARD).
    aborted: bool,
    capture: Option<Capture>,
    /// Bumped each time a capture starts (to tell whether a timeout is about the current one).
    capture_gen: u64,
    /// The key that ended a capture: its release is kept from the app too.
    swallow_up: Option<u16>,
}

impl Matcher {
    pub const fn new(binding: Binding) -> Self {
        Self { binding, held: None, aborted: false, capture: None, capture_gen: 0, swallow_up: None }
    }

    pub fn binding(&self) -> Binding {
        self.binding
    }

    pub fn set_binding(&mut self, b: Binding) {
        self.binding = b;
        self.held = None;
        self.aborted = false;
    }

    pub fn set_capture(&mut self, on: bool) {
        if on {
            self.capture = Some(Capture::default());
            self.capture_gen += 1;
            self.held = None;
        } else {
            self.capture = None;
        }
    }

    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    pub fn capture_gen(&self) -> u64 {
        self.capture_gen
    }

    /// Keys must be able to be kept from apps (macOS: an active event tap instead of a
    /// listen-only one, which sits in the path of every keystroke, so only when needed).
    pub fn wants_swallow(&self) -> bool {
        self.capture.is_some() || matches!(self.binding, Binding::Chord { .. })
    }

    pub fn feed(&mut self, k: RawKey, at: Instant, class: KeyClass) -> Verdict {
        if self.capture.is_some() {
            return self.feed_capture(k, class);
        }
        if let RawKey::Up { code } = k
            && self.swallow_up == Some(code)
        {
            self.swallow_up = None;
            return Verdict::swallow(None);
        }
        if matches!(k, RawKey::Down { .. }) && class == KeyClass::Escape {
            return Verdict::pass(Some(HotkeyEvent::Escape));
        }
        match self.binding {
            Binding::Modifier { key: bound } => match k {
                RawKey::Mod { key, down: true } if key == bound => {
                    if self.held.is_some() {
                        return Verdict::pass(None); // auto-repeat (Windows)
                    }
                    self.held = Some(at);
                    self.aborted = false;
                    Verdict::pass(Some(HotkeyEvent::TalkDown))
                }
                RawKey::Mod { key, down: false } if key == bound => match self.held.take() {
                    Some(_) if !self.aborted => Verdict::pass(Some(HotkeyEvent::TalkUp)),
                    _ => Verdict::pass(None),
                },
                RawKey::Down { repeat: false, .. } => match self.held {
                    Some(t) if !self.aborted && at.saturating_duration_since(t) < TYPING_GUARD => {
                        self.aborted = true;
                        Verdict::pass(Some(HotkeyEvent::TalkAborted))
                    }
                    _ => Verdict::pass(None),
                },
                _ => Verdict::pass(None),
            },
            Binding::Chord { code: bound, mods: bound_mods } => match k {
                RawKey::Down { code, .. } if code == bound && self.held.is_some() => Verdict::swallow(None),
                RawKey::Down { code, mods, repeat: false } if code == bound && mods == bound_mods => {
                    self.held = Some(at);
                    Verdict::swallow(Some(HotkeyEvent::TalkDown))
                }
                RawKey::Up { code } if code == bound && self.held.take().is_some() => {
                    Verdict::swallow(Some(HotkeyEvent::TalkUp))
                }
                _ => Verdict::pass(None),
            },
        }
    }

    fn feed_capture(&mut self, k: RawKey, class: KeyClass) -> Verdict {
        let Some(c) = self.capture.as_mut() else { return Verdict::pass(None) };
        let event = match k {
            RawKey::Down { .. } if class == KeyClass::Escape => {
                self.capture = None;
                Some(HotkeyEvent::CaptureCancelled)
            }
            RawKey::Down { repeat: true, .. } | RawKey::Up { .. } => None,
            RawKey::Down { code, mods, repeat: false } => {
                c.lone = None;
                let b = Binding::Chord { code, mods };
                match b.validate(class) {
                    Ok(()) => {
                        self.capture = None;
                        self.set_binding(b);
                        self.swallow_up = Some(code);
                        Some(HotkeyEvent::Captured(b))
                    }
                    Err(r) => Some(HotkeyEvent::CaptureRejected(r)),
                }
            }
            RawKey::Mod { key, down: true } => {
                c.lone = match c.lone {
                    Some(l) if l == key => Some(key), // auto-repeat (Windows)
                    _ if c.down == 0 => Some(key),
                    _ => None,
                };
                c.down |= key.bit();
                None
            }
            RawKey::Mod { key, down: false } => {
                c.down &= !key.bit();
                if c.lone == Some(key) && c.down == 0 {
                    c.lone = None;
                    let b = Binding::Modifier { key };
                    match b.validate(KeyClass::Other) {
                        Ok(()) => {
                            self.capture = None;
                            self.set_binding(b);
                            Some(HotkeyEvent::Captured(b))
                        }
                        Err(r) => Some(HotkeyEvent::CaptureRejected(r)),
                    }
                } else {
                    None
                }
            }
        };
        Verdict::swallow(event)
    }
}

/// The one matcher, shared by the keyboard hook thread and the app.
pub fn lock() -> MutexGuard<'static, Matcher> {
    static MATCHER: Mutex<Matcher> = Mutex::new(Matcher::new(Binding::DEFAULT));
    MATCHER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The current talk key, for the menu, the pill and Settings ("Right ⌥", "⌃⌥Space").
pub fn label() -> String {
    lock().binding().label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const SPACE: u16 = 49;
    const A: u16 = 0;
    const ESC: u16 = 53;
    const F5: u16 = 96;
    const CTRL_OPT: Mods = Mods { control: true, option: true, shift: false, command: false };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn modifier(key: ModKey) -> Binding {
        Binding::Modifier { key }
    }

    fn down(code: u16, mods: Mods) -> RawKey {
        RawKey::Down { code, mods, repeat: false }
    }

    fn class(code: u16) -> KeyClass {
        match code {
            ESC => KeyClass::Escape,
            F5 => KeyClass::Function,
            _ => KeyClass::Other,
        }
    }

    /// Feeds `k` at `t0 + after` and returns the verdict.
    fn feed(m: &mut Matcher, t0: Instant, after: u64, k: RawKey) -> Verdict {
        let c = match k {
            RawKey::Down { code, .. } | RawKey::Up { code } => class(code),
            RawKey::Mod { .. } => KeyClass::Other,
        };
        m.feed(k, t0 + ms(after), c)
    }

    fn ev(e: HotkeyEvent) -> Verdict {
        Verdict { event: Some(e), swallow: false }
    }

    fn swallowed(e: Option<HotkeyEvent>) -> Verdict {
        Verdict { event: e, swallow: true }
    }

    const NOTHING: Verdict = Verdict { event: None, swallow: false };

    #[test]
    fn serde_round_trips() {
        let chord = Binding::Chord { code: SPACE, mods: CTRL_OPT };
        for b in [Binding::DEFAULT, modifier(ModKey::Fn), chord] {
            let s = serde_json::to_string(&b).unwrap();
            assert_eq!(serde_json::from_str::<Binding>(&s).unwrap(), b, "{s}");
        }
        assert_eq!(serde_json::to_value(Binding::DEFAULT).unwrap(), serde_json::json!({"type": "modifier", "key": "right_option"}));
        assert_eq!(serde_json::to_value(chord).unwrap()["type"], "chord");
        for m in [TalkMode::HoldOrTap, TalkMode::Hold, TalkMode::Toggle] {
            let s = serde_json::to_string(&m).unwrap();
            assert_eq!(serde_json::from_str::<TalkMode>(&s).unwrap(), m);
        }
        assert_eq!(serde_json::to_string(&TalkMode::HoldOrTap).unwrap(), "\"hold_or_tap\"");
        assert_eq!(ModKey::from_id(&ModKey::RightCommand.id()), Some(ModKey::RightCommand));
    }

    #[test]
    fn old_config_keeps_todays_key() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.talk_key, Binding::DEFAULT);
        assert_eq!(c.talk_mode, TalkMode::HoldOrTap);
        let c: Config = serde_json::from_str(r#"{"talk_key":"garbage","talk_mode":7,"mixed_english":false}"#).unwrap();
        assert_eq!(c.talk_key, Binding::DEFAULT);
        assert_eq!(c.talk_mode, TalkMode::HoldOrTap);
        assert!(!c.mixed_english);
        let c: Config = serde_json::from_str(r#"{"talk_key":{"type":"chord","code":49,"mods":{"control":true}},"talk_mode":"toggle"}"#).unwrap();
        assert_eq!(c.talk_key, Binding::Chord { code: SPACE, mods: Mods { control: true, ..Mods::default() } });
        assert_eq!(c.talk_mode, TalkMode::Toggle);
    }

    #[test]
    fn validate_rules() {
        let none = Mods::default();
        let shift = Mods { shift: true, ..none };
        assert_eq!(Binding::Chord { code: ESC, mods: CTRL_OPT }.validate(KeyClass::Escape), Err(Reject::Escape));
        assert_eq!(Binding::Chord { code: A, mods: none }.validate(KeyClass::Other), Err(Reject::NeedsModifier));
        assert_eq!(Binding::Chord { code: A, mods: shift }.validate(KeyClass::Other), Err(Reject::NeedsModifier));
        assert_eq!(Binding::Chord { code: A, mods: CTRL_OPT }.validate(KeyClass::Other), Ok(()));
        assert_eq!(Binding::Chord { code: F5, mods: none }.validate(KeyClass::Function), Ok(()));
        assert_eq!(modifier(ModKey::LeftShift).validate(KeyClass::Other), Ok(()));
        #[cfg(windows)]
        assert_eq!(modifier(ModKey::Fn).validate(KeyClass::Other), Err(Reject::Unsupported));
        #[cfg(not(windows))]
        assert_eq!(modifier(ModKey::Fn).validate(KeyClass::Other), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn labels() {
        assert_eq!(Binding::DEFAULT.label(), "Right ⌥");
        assert_eq!(Binding::Chord { code: SPACE, mods: CTRL_OPT }.label(), "⌃⌥Space");
        assert_eq!(Binding::Chord { code: F5, mods: Mods::default() }.label(), "F5");
        assert_eq!(Binding::Chord { code: SPACE, mods: CTRL_OPT }.usable(), Binding::Chord { code: SPACE, mods: CTRL_OPT });
        assert_eq!(Binding::Chord { code: A, mods: Mods::default() }.usable(), Binding::DEFAULT);
    }

    #[test]
    fn modifier_down_up() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        let opt = |down| RawKey::Mod { key: ModKey::RightOption, down };
        assert_eq!(feed(&mut m, t, 0, opt(true)), ev(HotkeyEvent::TalkDown));
        assert_eq!(feed(&mut m, t, 30, opt(true)), NOTHING); // repeat
        assert_eq!(feed(&mut m, t, 900, opt(false)), ev(HotkeyEvent::TalkUp));
        assert_eq!(feed(&mut m, t, 950, opt(false)), NOTHING);
        assert!(!m.wants_swallow());
    }

    #[test]
    fn modifier_wrong_side_ignored() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        assert_eq!(feed(&mut m, t, 0, RawKey::Mod { key: ModKey::LeftOption, down: true }), NOTHING);
        assert_eq!(feed(&mut m, t, 500, RawKey::Mod { key: ModKey::LeftOption, down: false }), NOTHING);
        assert_eq!(feed(&mut m, t, 600, RawKey::Mod { key: ModKey::RightCommand, down: true }), NOTHING);
    }

    #[test]
    fn typing_guard() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        let opt = |down| RawKey::Mod { key: ModKey::RightOption, down };
        let e = down(14, Mods { option: true, ..Mods::default() });
        // ⌥+e typed quickly: the recording is dropped and the release says nothing.
        feed(&mut m, t, 0, opt(true));
        assert_eq!(feed(&mut m, t, 120, e), ev(HotkeyEvent::TalkAborted));
        assert_eq!(feed(&mut m, t, 140, e), NOTHING);
        assert_eq!(feed(&mut m, t, 200, opt(false)), NOTHING);
        // A key pressed later in a long hold is ignored.
        feed(&mut m, t, 1000, opt(true));
        assert_eq!(feed(&mut m, t, 1600, e), NOTHING);
        assert_eq!(feed(&mut m, t, 2000, opt(false)), ev(HotkeyEvent::TalkUp));
        // Repeats of a key held from before don't count.
        feed(&mut m, t, 3000, opt(true));
        assert_eq!(feed(&mut m, t, 3050, RawKey::Down { code: 14, mods: Mods::default(), repeat: true }), NOTHING);
        assert_eq!(feed(&mut m, t, 3900, opt(false)), ev(HotkeyEvent::TalkUp));
    }

    #[test]
    fn chord_down_repeat_up() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: CTRL_OPT }), Instant::now());
        assert!(m.wants_swallow());
        assert_eq!(feed(&mut m, t, 0, RawKey::Mod { key: ModKey::LeftControl, down: true }), NOTHING);
        assert_eq!(feed(&mut m, t, 10, down(SPACE, CTRL_OPT)), swallowed(Some(HotkeyEvent::TalkDown)));
        assert_eq!(feed(&mut m, t, 400, RawKey::Down { code: SPACE, mods: CTRL_OPT, repeat: true }), swallowed(None));
        // Released after the modifiers: still the end of the press.
        assert_eq!(feed(&mut m, t, 900, RawKey::Up { code: SPACE }), swallowed(Some(HotkeyEvent::TalkUp)));
        // A plain space types as usual.
        assert_eq!(feed(&mut m, t, 1000, down(SPACE, Mods::default())), NOTHING);
        assert_eq!(feed(&mut m, t, 1050, RawKey::Up { code: SPACE }), NOTHING);
    }

    #[test]
    fn chord_needs_exact_mods() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: CTRL_OPT }), Instant::now());
        let extra = Mods { command: true, ..CTRL_OPT };
        assert_eq!(feed(&mut m, t, 0, down(SPACE, extra)), NOTHING);
        assert_eq!(feed(&mut m, t, 50, RawKey::Up { code: SPACE }), NOTHING);
    }

    #[test]
    fn escape_passes_through() {
        for b in [Binding::DEFAULT, Binding::Chord { code: SPACE, mods: CTRL_OPT }] {
            let (mut m, t) = (Matcher::new(b), Instant::now());
            assert_eq!(feed(&mut m, t, 0, down(ESC, Mods::default())), ev(HotkeyEvent::Escape));
        }
        // Esc while holding the talk modifier cancels; it isn't "typing".
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        feed(&mut m, t, 0, RawKey::Mod { key: ModKey::RightOption, down: true });
        assert_eq!(feed(&mut m, t, 100, down(ESC, Mods::default())), ev(HotkeyEvent::Escape));
    }

    #[test]
    fn capture_lone_modifier() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        assert!(m.wants_swallow());
        let cmd = |down| RawKey::Mod { key: ModKey::RightCommand, down };
        assert_eq!(feed(&mut m, t, 0, cmd(true)), swallowed(None));
        assert_eq!(feed(&mut m, t, 200, cmd(false)), swallowed(Some(HotkeyEvent::Captured(modifier(ModKey::RightCommand)))));
        assert!(!m.capturing());
        assert_eq!(m.binding(), modifier(ModKey::RightCommand));
        assert!(!m.wants_swallow());
        // Two modifiers together are not a lone modifier.
        m.set_capture(true);
        feed(&mut m, t, 1000, RawKey::Mod { key: ModKey::LeftShift, down: true });
        feed(&mut m, t, 1050, RawKey::Mod { key: ModKey::RightShift, down: true });
        assert_eq!(feed(&mut m, t, 1100, RawKey::Mod { key: ModKey::LeftShift, down: false }), swallowed(None));
        assert_eq!(feed(&mut m, t, 1150, RawKey::Mod { key: ModKey::RightShift, down: false }), swallowed(None));
        assert!(m.capturing());
    }

    #[test]
    fn capture_chord() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        let chord = Binding::Chord { code: SPACE, mods: CTRL_OPT };
        assert_eq!(feed(&mut m, t, 0, RawKey::Mod { key: ModKey::LeftControl, down: true }), swallowed(None));
        assert_eq!(feed(&mut m, t, 10, RawKey::Mod { key: ModKey::LeftOption, down: true }), swallowed(None));
        assert_eq!(feed(&mut m, t, 20, down(SPACE, CTRL_OPT)), swallowed(Some(HotkeyEvent::Captured(chord))));
        assert_eq!(m.binding(), chord);
        // The key's release doesn't start anything and doesn't reach the app.
        assert_eq!(feed(&mut m, t, 100, RawKey::Up { code: SPACE }), swallowed(None));
        assert_eq!(feed(&mut m, t, 150, RawKey::Mod { key: ModKey::LeftOption, down: false }), NOTHING);
        assert_eq!(feed(&mut m, t, 160, RawKey::Mod { key: ModKey::LeftControl, down: false }), NOTHING);
        assert_eq!(feed(&mut m, t, 500, down(SPACE, CTRL_OPT)), swallowed(Some(HotkeyEvent::TalkDown)));
    }

    #[test]
    fn capture_rejects_bare_letter_and_stays() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        assert_eq!(feed(&mut m, t, 0, down(A, Mods::default())), swallowed(Some(HotkeyEvent::CaptureRejected(Reject::NeedsModifier))));
        assert_eq!(feed(&mut m, t, 50, RawKey::Up { code: A }), swallowed(None));
        assert!(m.capturing());
        assert_eq!(m.binding(), Binding::DEFAULT);
        // ⇧ pressed for ⇧A is not then taken as a lone modifier.
        feed(&mut m, t, 100, RawKey::Mod { key: ModKey::LeftShift, down: true });
        feed(&mut m, t, 120, down(A, Mods { shift: true, ..Mods::default() }));
        assert_eq!(feed(&mut m, t, 200, RawKey::Mod { key: ModKey::LeftShift, down: false }), swallowed(None));
        assert!(m.capturing());
        assert_eq!(feed(&mut m, t, 300, down(F5, Mods::default())), swallowed(Some(HotkeyEvent::Captured(Binding::Chord { code: F5, mods: Mods::default() }))));
    }

    #[test]
    fn escape_cancels_capture() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        let g = m.capture_gen();
        assert_eq!(feed(&mut m, t, 0, down(ESC, Mods::default())), swallowed(Some(HotkeyEvent::CaptureCancelled)));
        assert!(!m.capturing());
        assert_eq!(m.binding(), Binding::DEFAULT);
        m.set_capture(true);
        assert_ne!(m.capture_gen(), g);
        // While capturing, the talk key does nothing.
        assert_eq!(feed(&mut m, t, 100, RawKey::Mod { key: ModKey::RightOption, down: true }), swallowed(None));
    }

    #[test]
    fn release_by_mode() {
        assert_eq!(on_release(TalkMode::HoldOrTap, ms(100)), Release::Handsfree);
        assert_eq!(on_release(TalkMode::HoldOrTap, ms(800)), Release::Stop);
        assert_eq!(on_release(TalkMode::Hold, ms(100)), Release::Stop);
        assert_eq!(on_release(TalkMode::Hold, ms(800)), Release::Stop);
        assert_eq!(on_release(TalkMode::Toggle, ms(100)), Release::Ignore);
        assert_eq!(on_release(TalkMode::Toggle, ms(800)), Release::Ignore);
        assert!(starts_handsfree(TalkMode::Toggle));
        assert!(!starts_handsfree(TalkMode::HoldOrTap) && !starts_handsfree(TalkMode::Hold));
    }
}

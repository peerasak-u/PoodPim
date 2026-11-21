//! The talk key: which key it is and how a press is read (both settings), and the matcher that
//! turns the macOS keyboard tap's key events into talk events, decides which keys are kept from
//! apps (a chord such as ⌃ + ⌥ + Space), and records a new talk or cancel key from Settings.
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

/// Modifiers held with a chord's key, either side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    /// One modifier key on its own.
    Modifier { key: ModKey },
    /// A key with exactly these modifiers (macOS virtual keycode), kept from apps.
    Chord { code: u16, mods: Mods },
}

impl Binding {
    pub const DEFAULT: Binding = Binding::Modifier { key: ModKey::RightOption };

    /// The key in use: on Windows always Right Alt (its keyboard hook is fixed).
    pub fn usable(self) -> Binding {
        if cfg!(windows) { Binding::DEFAULT } else { self }
    }

    /// For the menu, the pill and Settings: "Right ⌥", "⌃ + ⌥ + Space", "F5".
    pub fn label(&self) -> String {
        match *self {
            Binding::Modifier { key: ModKey::RightOption } => platform::TALK_KEY_LABEL.into(),
            Binding::Modifier { key: ModKey::RightControl } => "Right ⌃".into(),
            Binding::Modifier { key: ModKey::Fn } => "fn".into(),
            Binding::Chord { code, mods } => Chord { code, mods }.label(),
        }
    }
}

/// A key with exactly these modifiers (macOS virtual keycode): the cancel key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub code: u16,
    #[serde(default)]
    pub mods: Mods,
}

impl Chord {
    /// Esc on its own: the default cancel key, the one Windows always uses, and the key that
    /// stops recording a new key in Settings.
    pub const ESCAPE: Chord = Chord { code: 53, mods: Mods { control: false, option: false, shift: false, command: false } };

    /// "Esc", "⌥ + ⌫", "⌃ + ⌥ + Space": the modifiers, then the key, joined with " + ".
    pub fn label(&self) -> String {
        let m = self.mods;
        let names = [(m.control, "⌃"), (m.option, "⌥"), (m.shift, "⇧"), (m.command, "⌘")];
        let mut parts: Vec<String> = names.iter().filter(|(on, _)| *on).map(|(_, n)| n.to_string()).collect();
        parts.push(platform::key_name(self.code));
        parts.join(" + ")
    }

    /// The cancel key in use: on Windows always Esc (its keyboard hook is fixed).
    pub fn usable(self) -> Chord {
        if cfg!(windows) { Chord::ESCAPE } else { self }
    }
}

impl Default for Chord {
    fn default() -> Self {
        Self::ESCAPE
    }
}

/// Which key Settings is recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Talk,
    Cancel,
}

/// A key as far as recording it as the talk key goes (platform::key_kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Escape,
    Space,
    Tab,
    /// A letter key (its name on a US keyboard).
    Letter(char),
    /// F1–F20 and other keys that never type a character: fine on their own.
    Function,
    Other,
}

/// Why a key can't be the talk (or cancel) key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Esc stops the recording of a key, so it can't be recorded.
    Escape,
    /// It is the talk key.
    TalkKey,
    /// It is the cancel key.
    CancelKey,
    /// A typing key needs ⌃, ⌥ or ⌘ with it.
    NeedsModifier,
    /// A shortcut macOS or every app already uses (⌘Space, ⌘Tab, ⌘C…).
    System,
}

/// Whether a key pressed with `mods` may be the talk key.
pub fn check_chord(kind: KeyKind, mods: Mods) -> Result<(), Reject> {
    let only = |m: Mods| mods == m;
    let cmd = Mods { command: true, ..Mods::default() };
    let ctrl = Mods { control: true, ..Mods::default() };
    let ctrl_cmd = Mods { control: true, command: true, ..Mods::default() };
    match kind {
        KeyKind::Escape => Err(Reject::Escape),
        // App switcher, Spotlight, input source, emoji & symbols.
        KeyKind::Tab | KeyKind::Space if only(cmd) => Err(Reject::System),
        KeyKind::Space if only(ctrl) || only(ctrl_cmd) => Err(Reject::System),
        // ⌘Q, ⌘W, ⌘C, ⌘V, ⌘X, ⌘Z, ⌘A… and Lock Screen.
        KeyKind::Letter(_) if only(cmd) => Err(Reject::System),
        KeyKind::Letter('Q') if only(ctrl_cmd) => Err(Reject::System),
        KeyKind::Function => Ok(()),
        // ⇧ alone doesn't do: ⇧+letter is typing.
        _ if !(mods.control || mods.option || mods.command) => Err(Reject::NeedsModifier),
        _ => Ok(()),
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
    /// Esc went down (with any modifiers).
    Escape,
    /// Any other key went down; `repeat`: the OS repeating a key held down.
    Down { code: u16, mods: Mods, kind: KeyKind, repeat: bool },
    Up { code: u16 },
}

/// What to do with a key event.
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

pub struct Matcher {
    binding: Binding,
    cancel: Chord,
    /// Recording or finishing a dictation: a cancel key other than Esc is kept from apps then
    /// (Esc reaches them, as it always has).
    dictating: bool,
    /// Which key Settings is recording, while `capturing`.
    target: Target,
    /// Recording the talk key: this modifier is down with no other key yet. Let go like that,
    /// it becomes the talk key on its own.
    capture_mod: Option<ModKey>,
    /// The talk key is down, since then.
    held: Option<Instant>,
    /// TalkAborted was already sent for this press.
    aborted: bool,
    /// Settings is recording a new talk key (bumped per recording, to tell a timeout's apart).
    capturing: bool,
    capture_gen: u64,
    /// The key that ended a recording or started the talk key: its release is kept from apps too.
    swallow_up: Option<u16>,
}

impl Matcher {
    pub const fn new(binding: Binding) -> Self {
        Self {
            binding,
            cancel: Chord::ESCAPE,
            dictating: false,
            target: Target::Talk,
            capture_mod: None,
            held: None,
            aborted: false,
            capturing: false,
            capture_gen: 0,
            swallow_up: None,
        }
    }

    pub fn binding(&self) -> Binding {
        self.binding
    }

    pub fn set_binding(&mut self, b: Binding) {
        self.binding = b;
        self.held = None;
        self.swallow_up = None;
    }

    pub fn cancel(&self) -> Chord {
        self.cancel
    }

    pub fn set_cancel(&mut self, c: Chord) {
        self.cancel = c;
    }

    pub fn set_dictating(&mut self, on: bool) {
        self.dictating = on;
    }

    /// What is being recorded (meaningful while `capturing`).
    pub fn target(&self) -> Target {
        self.target
    }

    /// Starts recording a new `target` key; returns the recording's number.
    pub fn start_capture(&mut self, target: Target) -> u64 {
        self.target = target;
        self.set_capture(true)
    }

    /// Starts or stops recording a new key; returns the recording's number.
    pub fn set_capture(&mut self, on: bool) -> u64 {
        self.capturing = on;
        self.capture_mod = None;
        self.held = None;
        self.swallow_up = None;
        if on {
            self.capture_gen += 1;
        }
        self.capture_gen
    }

    pub fn capturing(&self) -> bool {
        self.capturing
    }

    pub fn capture_gen(&self) -> u64 {
        self.capture_gen
    }

    /// Keys must be able to be kept from apps: the macOS tap is then an active one (in the path
    /// of every keystroke) instead of listen-only, so only for a chord or while recording a key.
    pub fn wants_swallow(&self) -> bool {
        self.capturing || matches!(self.binding, Binding::Chord { .. }) || (self.dictating && self.cancel != Chord::ESCAPE)
    }

    pub fn feed(&mut self, k: RawKey, at: Instant) -> Verdict {
        if self.capturing {
            return self.feed_capture(k);
        }
        if let RawKey::Up { code } = k
            && self.swallow_up == Some(code)
        {
            self.swallow_up = None;
            return Verdict::swallow(None);
        }
        if k == RawKey::Escape {
            return Verdict::pass((self.cancel == Chord::ESCAPE).then_some(HotkeyEvent::Escape));
        }
        if let RawKey::Down { code, mods, repeat, .. } = k
            && code == self.cancel.code
            && mods == self.cancel.mods
        {
            // Kept from apps only while there is something to cancel; otherwise it does its usual job.
            if !self.dictating {
                return Verdict::pass(None);
            }
            self.swallow_up = Some(code);
            return Verdict::swallow((!repeat).then_some(HotkeyEvent::Escape));
        }
        match self.binding {
            Binding::Modifier { key: bound } => Verdict::pass(match k {
                // macOS never repeats a modifier, so a second "down" means the release was
                // missed (the tap was off for a moment, or a password field hid it): a new press.
                RawKey::Mod { key, down: true } if key == bound => {
                    self.held = Some(at);
                    self.aborted = false;
                    Some(HotkeyEvent::TalkDown)
                }
                RawKey::Mod { key, down: false } if key == bound && self.held.is_some() => {
                    self.held = None;
                    Some(HotkeyEvent::TalkUp)
                }
                RawKey::Down { repeat: false, .. } => match self.held {
                    Some(t) if !self.aborted && at.saturating_duration_since(t) < TYPING_GUARD => {
                        self.aborted = true;
                        Some(HotkeyEvent::TalkAborted)
                    }
                    _ => None,
                },
                _ => None,
            }),
            Binding::Chord { code: bound, mods: bound_mods } => match k {
                RawKey::Down { code, repeat: true, .. } if code == bound && self.held.is_some() => Verdict::swallow(None),
                // Not a repeat: a new press, even if the last release was missed.
                RawKey::Down { code, mods, repeat: false, .. } if code == bound && mods == bound_mods => {
                    self.held = Some(at);
                    Verdict::swallow(Some(HotkeyEvent::TalkDown))
                }
                RawKey::Down { code, repeat: false, .. } if code == bound => {
                    self.held = None; // the same key typed on its own (a plain space)
                    Verdict::pass(None)
                }
                // Let go of the key, before or after its modifiers.
                RawKey::Up { code } if code == bound && self.held.take().is_some() => {
                    Verdict::swallow(Some(HotkeyEvent::TalkUp))
                }
                _ => Verdict::pass(None),
            },
        }
    }

    /// While recording a new key, every key but the modifiers is kept from apps. A talk-key
    /// modifier (Right ⌥, Right ⌃, fn) pressed and let go on its own becomes the talk key.
    fn feed_capture(&mut self, k: RawKey) -> Verdict {
        match k {
            RawKey::Mod { key, down: true } => {
                self.capture_mod = (self.target == Target::Talk).then_some(key);
                Verdict::pass(None)
            }
            RawKey::Mod { key, down: false } if self.capture_mod == Some(key) => {
                let b = Binding::Modifier { key };
                self.set_capture(false);
                self.set_binding(b);
                Verdict::pass(Some(HotkeyEvent::Captured(b)))
            }
            RawKey::Mod { .. } => Verdict::pass(None),
            RawKey::Escape => {
                self.set_capture(false);
                Verdict::swallow(Some(HotkeyEvent::CaptureCancelled))
            }
            RawKey::Down { repeat: true, .. } | RawKey::Up { .. } => Verdict::swallow(None),
            RawKey::Down { code, mods, kind, repeat: false } => {
                self.capture_mod = None; // a combination, not the modifier alone
                let chord = Chord { code, mods };
                let taken = match self.target {
                    Target::Talk if chord == self.cancel => Err(Reject::CancelKey),
                    Target::Cancel if self.binding == (Binding::Chord { code, mods }) => Err(Reject::TalkKey),
                    _ => check_chord(kind, mods),
                };
                let event = match taken {
                    Ok(()) => {
                        self.set_capture(false);
                        match self.target {
                            Target::Talk => {
                                let b = Binding::Chord { code, mods };
                                self.set_binding(b);
                                Some(HotkeyEvent::Captured(b))
                            }
                            Target::Cancel => {
                                self.cancel = chord;
                                Some(HotkeyEvent::CancelCaptured(chord))
                            }
                        }
                    }
                    Err(r) => Some(HotkeyEvent::CaptureRejected(r)),
                };
                self.swallow_up = Some(code);
                Verdict::swallow(event)
            }
        }
    }
}

/// The matcher the macOS keyboard tap feeds (the app sets its key from the config).
pub fn lock() -> MutexGuard<'static, Matcher> {
    static MATCHER: Mutex<Matcher> = Mutex::new(Matcher::new(Binding::DEFAULT));
    MATCHER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Changes the matcher with `f`; if the tap must change kind (listen-only / active) for it,
/// wakes the keyboard thread to make the new tap at once.
pub fn update<R>(f: impl FnOnce(&mut Matcher) -> R) -> R {
    let mut m = lock();
    let before = m.wants_swallow();
    let r = f(&mut m);
    if m.wants_swallow() != before {
        platform::wake_hotkey_listener();
    }
    r
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

    const SPACE: u16 = 49;
    const E: u16 = 14;
    const NONE: Mods = Mods { control: false, option: false, shift: false, command: false };
    const CTRL_OPT: Mods = Mods { control: true, option: true, shift: false, command: false };
    const KEY: RawKey = RawKey::Down { code: E, mods: NONE, kind: KeyKind::Letter('E'), repeat: false };

    fn down(code: u16, mods: Mods, kind: KeyKind) -> RawKey {
        RawKey::Down { code, mods, kind, repeat: false }
    }

    fn space(mods: Mods) -> RawKey {
        down(SPACE, mods, KeyKind::Space)
    }

    /// Feeds `k` to `m` at `t0 + ms`: the event, checking that the key reaches apps.
    fn at(m: &mut Matcher, t0: Instant, ms: u64, k: RawKey) -> Option<HotkeyEvent> {
        let v = m.feed(k, t0 + Duration::from_millis(ms));
        assert!(!v.swallow, "{k:?} was kept from apps");
        v.event
    }

    /// Feeds `k`, checking that it is kept from apps.
    fn kept(m: &mut Matcher, t0: Instant, ms: u64, k: RawKey) -> Option<HotkeyEvent> {
        let v = m.feed(k, t0 + Duration::from_millis(ms));
        assert!(v.swallow, "{k:?} reached apps");
        v.event
    }

    #[test]
    fn serde_round_trips() {
        let chord = Binding::Chord { code: SPACE, mods: CTRL_OPT };
        for b in [Binding::Modifier { key: ModKey::RightOption }, Binding::Modifier { key: ModKey::Fn }, chord] {
            let s = serde_json::to_string(&b).unwrap();
            assert_eq!(serde_json::from_str::<Binding>(&s).unwrap(), b, "{s}");
        }
        assert_eq!(serde_json::to_value(Binding::DEFAULT).unwrap(), serde_json::json!({"type": "modifier", "key": "right_option"}));
        assert_eq!(
            serde_json::to_value(chord).unwrap(),
            serde_json::json!({"type": "chord", "code": 49, "mods": {"control": true, "option": true, "shift": false, "command": false}})
        );
        // Mods left out are off.
        let b: Binding = serde_json::from_str(r#"{"type":"chord","code":96,"mods":{}}"#).unwrap();
        assert_eq!(b, Binding::Chord { code: 96, mods: NONE });
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
        assert_eq!(at(&mut m, t, 3050, RawKey::Down { code: E, mods: NONE, kind: KeyKind::Letter('E'), repeat: true }), None);
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

    #[test]
    fn modifier_is_never_kept_from_apps() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        assert!(!m.wants_swallow());
        at(&mut m, t, 0, opt(true));
        at(&mut m, t, 100, space(Mods { option: true, ..NONE }));
        at(&mut m, t, 150, RawKey::Up { code: SPACE });
        assert_eq!(at(&mut m, t, 200, RawKey::Escape), Some(HotkeyEvent::Escape));
        at(&mut m, t, 300, opt(false));
    }

    #[test]
    fn chord_down_repeat_up() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: CTRL_OPT }), Instant::now());
        assert!(m.wants_swallow());
        assert_eq!(at(&mut m, t, 0, ctrl(true)), None);
        assert_eq!(kept(&mut m, t, 10, space(CTRL_OPT)), Some(HotkeyEvent::TalkDown));
        let repeat = RawKey::Down { code: SPACE, mods: CTRL_OPT, kind: KeyKind::Space, repeat: true };
        assert_eq!(kept(&mut m, t, 400, repeat), None);
        // No typing guard for a chord: other keys pass and don't drop anything.
        assert_eq!(at(&mut m, t, 450, KEY), None);
        // Let go after the modifiers: still the end of the press.
        assert_eq!(kept(&mut m, t, 900, RawKey::Up { code: SPACE }), Some(HotkeyEvent::TalkUp));
        // A plain space, or one with other modifiers, types as usual.
        assert_eq!(at(&mut m, t, 1000, space(NONE)), None);
        assert_eq!(at(&mut m, t, 1050, RawKey::Up { code: SPACE }), None);
        assert_eq!(at(&mut m, t, 1100, space(Mods { command: true, ..CTRL_OPT })), None);
        assert_eq!(at(&mut m, t, 1150, RawKey::Up { code: SPACE }), None);
        assert_eq!(at(&mut m, t, 1200, RawKey::Escape), Some(HotkeyEvent::Escape));
    }

    #[test]
    fn chord_missed_release() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: CTRL_OPT }), Instant::now());
        assert_eq!(kept(&mut m, t, 0, space(CTRL_OPT)), Some(HotkeyEvent::TalkDown));
        // The release was missed (password field): only a repeat is silent, a new press starts.
        assert_eq!(kept(&mut m, t, 5000, space(CTRL_OPT)), Some(HotkeyEvent::TalkDown));
        assert_eq!(kept(&mut m, t, 5500, RawKey::Up { code: SPACE }), Some(HotkeyEvent::TalkUp));
        // Stale, then a plain space: typed, and its release isn't a talk event.
        kept(&mut m, t, 6000, space(CTRL_OPT));
        assert_eq!(at(&mut m, t, 7000, space(NONE)), None);
        assert_eq!(at(&mut m, t, 7050, RawKey::Up { code: SPACE }), None);
        // Changing the key or recording one forgets a held key and a pending release.
        kept(&mut m, t, 8000, space(CTRL_OPT));
        m.set_binding(Binding::Chord { code: SPACE, mods: CTRL_OPT });
        assert_eq!(at(&mut m, t, 8100, RawKey::Up { code: SPACE }), None);
        kept(&mut m, t, 8200, space(CTRL_OPT));
        m.set_capture(true);
        m.set_capture(false);
        assert_eq!(at(&mut m, t, 8300, RawKey::Up { code: SPACE }), None);
        m.set_capture(true);
        kept(&mut m, t, 8400, space(CTRL_OPT)); // captured: its release would be kept…
        m.set_binding(Binding::DEFAULT);
        assert_eq!(at(&mut m, t, 8500, RawKey::Up { code: SPACE }), None); // …until the key changes
    }

    #[test]
    fn capture_chord() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        assert!(m.wants_swallow());
        let chord = Binding::Chord { code: SPACE, mods: CTRL_OPT };
        // Modifiers reach apps; held for a combination, they are not taken on their own.
        assert_eq!(at(&mut m, t, 0, opt(true)), None);
        assert_eq!(at(&mut m, t, 10, ctrl(true)), None);
        assert_eq!(kept(&mut m, t, 20, space(CTRL_OPT)), Some(HotkeyEvent::Captured(chord)));
        assert!(!m.capturing());
        assert_eq!(m.binding(), chord);
        // Its release doesn't start anything and doesn't reach the app either.
        assert_eq!(kept(&mut m, t, 100, RawKey::Up { code: SPACE }), None);
        assert_eq!(at(&mut m, t, 150, ctrl(false)), None);
        assert_eq!(at(&mut m, t, 160, opt(false)), None);
        assert_eq!(kept(&mut m, t, 500, space(CTRL_OPT)), Some(HotkeyEvent::TalkDown));
        // An F-key needs no modifier.
        m.set_capture(true);
        let f5 = Binding::Chord { code: 96, mods: NONE };
        assert_eq!(kept(&mut m, t, 1000, down(96, NONE, KeyKind::Function)), Some(HotkeyEvent::Captured(f5)));
    }

    #[test]
    fn capture_refuses_and_keeps_recording() {
        let cmd = Mods { command: true, ..NONE };
        let ctrl_m = Mods { control: true, ..NONE };
        let ctrl_cmd = Mods { control: true, command: true, ..NONE };
        let refused: &[(KeyKind, Mods, Reject)] = &[
            (KeyKind::Letter('A'), NONE, Reject::NeedsModifier),
            (KeyKind::Letter('A'), Mods { shift: true, ..NONE }, Reject::NeedsModifier),
            (KeyKind::Space, NONE, Reject::NeedsModifier),
            (KeyKind::Tab, cmd, Reject::System),
            (KeyKind::Space, cmd, Reject::System),
            (KeyKind::Space, ctrl_m, Reject::System),
            (KeyKind::Space, ctrl_cmd, Reject::System),
            (KeyKind::Letter('Q'), ctrl_cmd, Reject::System),
        ];
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_capture(true);
        let mut ms = 0;
        let mut feed_refused = |m: &mut Matcher, kind, mods, r| {
            ms += 100;
            assert_eq!(kept(m, t, ms, down(1, mods, kind)), Some(HotkeyEvent::CaptureRejected(r)), "{kind:?} {mods:?}");
            assert_eq!(kept(m, t, ms + 10, RawKey::Up { code: 1 }), None);
        };
        for &(kind, mods, r) in refused {
            feed_refused(&mut m, kind, mods, r);
        }
        for c in "QWCVXZAHM".chars() {
            feed_refused(&mut m, KeyKind::Letter(c), cmd, Reject::System);
        }
        feed_refused(&mut m, KeyKind::Escape, CTRL_OPT, Reject::Escape);
        assert!(m.capturing());
        assert_eq!(m.binding(), Binding::DEFAULT);
        // Allowed: ⌥Space, ⌃⌥Space, ⇧⌘K (⌘ with another modifier), ⌃⌘M.
        for mods in [Mods { option: true, ..NONE }, CTRL_OPT] {
            assert_eq!(check_chord(KeyKind::Space, mods), Ok(()));
        }
        assert_eq!(check_chord(KeyKind::Letter('K'), Mods { shift: true, command: true, ..NONE }), Ok(()));
        assert_eq!(check_chord(KeyKind::Letter('M'), ctrl_cmd), Ok(()));
        assert_eq!(check_chord(KeyKind::Other, Mods { option: true, ..NONE }), Ok(()));
    }

    const BACKSPACE: u16 = 51;
    const OPT: Mods = Mods { option: true, ..NONE };

    fn opt_backspace(repeat: bool) -> RawKey {
        RawKey::Down { code: BACKSPACE, mods: OPT, kind: KeyKind::Other, repeat }
    }

    #[test]
    fn custom_cancel_key_works_only_while_dictating() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        m.set_cancel(Chord { code: BACKSPACE, mods: OPT });
        assert!(!m.wants_swallow(), "no active tap while idle");
        // Idle: ⌥⌫ deletes a word as usual, and Esc no longer cancels.
        assert_eq!(at(&mut m, t, 0, opt_backspace(false)), None);
        assert_eq!(at(&mut m, t, 10, RawKey::Escape), None);
        // Dictating: ⌥⌫ cancels and is kept from apps, repeats and release included.
        m.set_dictating(true);
        assert!(m.wants_swallow());
        assert_eq!(kept(&mut m, t, 20, opt_backspace(false)), Some(HotkeyEvent::Escape));
        assert_eq!(kept(&mut m, t, 30, opt_backspace(true)), None);
        assert_eq!(kept(&mut m, t, 40, RawKey::Up { code: BACKSPACE }), None);
        // ⌫ without ⌥ is not the cancel key.
        assert_eq!(at(&mut m, t, 50, RawKey::Down { code: BACKSPACE, mods: NONE, kind: KeyKind::Other, repeat: false }), None);
        m.set_dictating(false);
        assert!(!m.wants_swallow());
    }

    #[test]
    fn records_a_cancel_key_but_not_the_talk_key() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: OPT }), Instant::now());
        m.start_capture(Target::Cancel);
        assert_eq!(kept(&mut m, t, 0, space(OPT)), Some(HotkeyEvent::CaptureRejected(Reject::TalkKey)));
        assert_eq!(kept(&mut m, t, 10, down(E, NONE, KeyKind::Letter('E'))), Some(HotkeyEvent::CaptureRejected(Reject::NeedsModifier)));
        let c = Chord { code: BACKSPACE, mods: OPT };
        assert_eq!(kept(&mut m, t, 20, opt_backspace(false)), Some(HotkeyEvent::CancelCaptured(c)));
        assert!(!m.capturing());
        assert_eq!(m.cancel(), c);
        assert_eq!(m.binding(), Binding::Chord { code: SPACE, mods: OPT }, "talk key unchanged");
        // And the talk key can't become the cancel key.
        kept(&mut m, t, 30, RawKey::Up { code: BACKSPACE });
        m.start_capture(Target::Talk);
        assert_eq!(kept(&mut m, t, 40, opt_backspace(false)), Some(HotkeyEvent::CaptureRejected(Reject::CancelKey)));
        assert_eq!(Chord { code: BACKSPACE, mods: OPT }.label(), "⌥ + Delete");
    }

    #[test]
    fn records_a_modifier_pressed_on_its_own() {
        let (mut m, t) = (Matcher::new(Binding::Chord { code: SPACE, mods: OPT }), Instant::now());
        m.start_capture(Target::Talk);
        // Right ⌥ held for a combination: not taken; the combination is judged instead.
        assert_eq!(at(&mut m, t, 0, opt(true)), None);
        assert_eq!(kept(&mut m, t, 10, down(E, OPT, KeyKind::Letter('E'))), Some(HotkeyEvent::Captured(Binding::Chord { code: E, mods: OPT })));
        assert_eq!(at(&mut m, t, 20, opt(false)), None);
        // Right ⌃ pressed and let go on its own: the talk key.
        kept(&mut m, t, 30, RawKey::Up { code: E });
        m.start_capture(Target::Talk);
        assert_eq!(at(&mut m, t, 40, ctrl(true)), None);
        assert_eq!(at(&mut m, t, 50, ctrl(false)), Some(HotkeyEvent::Captured(Binding::Modifier { key: ModKey::RightControl })));
        assert!(!m.capturing());
        // Not for the cancel key.
        m.start_capture(Target::Cancel);
        assert_eq!(at(&mut m, t, 60, opt(true)), None);
        assert_eq!(at(&mut m, t, 70, opt(false)), None);
        assert!(m.capturing());
    }

    #[test]
    fn escape_cancels_capture() {
        let (mut m, t) = (Matcher::new(Binding::DEFAULT), Instant::now());
        let g = m.set_capture(true);
        assert_eq!(kept(&mut m, t, 0, RawKey::Escape), Some(HotkeyEvent::CaptureCancelled));
        assert!(!m.capturing());
        assert!(!m.wants_swallow());
        assert_eq!(m.binding(), Binding::DEFAULT);
        assert_ne!(m.set_capture(true), g);
        // After the recording, Esc cancels dictation again and reaches apps.
        m.set_capture(false);
        assert_eq!(at(&mut m, t, 100, RawKey::Escape), Some(HotkeyEvent::Escape));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chord_labels() {
        assert_eq!(Binding::Chord { code: SPACE, mods: CTRL_OPT }.label(), "⌃ + ⌥ + Space");
        assert_eq!(Binding::Chord { code: 96, mods: NONE }.label(), "F5");
        assert_eq!(Binding::Chord { code: 40, mods: Mods { shift: true, command: true, ..NONE } }.label(), "⇧ + ⌘ + K");
        assert_eq!(Chord::ESCAPE.label(), "Esc");
        assert_eq!(platform::key_kind(SPACE), KeyKind::Space);
        assert_eq!(platform::key_kind(12), KeyKind::Letter('Q'));
        assert_eq!(platform::key_kind(53), KeyKind::Escape);
        assert_eq!(platform::key_kind(122), KeyKind::Function);
        assert_eq!(platform::key_kind(18), KeyKind::Other);
    }
}

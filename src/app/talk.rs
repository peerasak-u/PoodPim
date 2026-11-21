//! Settings › ปุ่มพูด: the talk key (a preset, or one recorded from the keyboard) and the press mode.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{App, Ui, UserEvent};
use crate::hotkey::{self, Binding, ModKey, Reject, TalkMode};
use crate::platform::{self, HotkeyEvent};

/// Recording a new talk key gives up after this.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// TalkAborted drops the recording only if it began this recently (the matcher sends it within
/// 500 ms of the press; a recording that started earlier was carried on, not started, by it).
pub(super) const ABORT_WITHIN: Duration = Duration::from_millis(600);

/// The tray menu's line on how to use the talk key.
pub(super) fn menu_hint(mode: TalkMode) -> String {
    let key = hotkey::label();
    match mode {
        TalkMode::HoldOrTap => format!("Hold {key} to talk · tap = hands-free · Esc = cancel"),
        TalkMode::Hold => format!("Hold {key} to talk · Esc = cancel"),
        TalkMode::Toggle => format!("Tap {key} to start, tap again to finish · Esc = cancel"),
    }
}

pub(super) fn reject_message(r: Reject) -> &'static str {
    match r {
        Reject::Escape => "Esc ใช้ยกเลิกอยู่แล้ว เลือกปุ่มอื่น",
        Reject::NeedsModifier if cfg!(windows) => "ปุ่มนี้ใช้พิมพ์ตัวอักษร ให้กดร่วมกับ Ctrl, Alt หรือ Win",
        Reject::NeedsModifier => "ปุ่มนี้ใช้พิมพ์ตัวอักษร ให้กดร่วมกับ ⌃ ⌥ หรือ ⌘",
        Reject::Unsupported => "ปุ่มนี้ใช้ไม่ได้บนเครื่องนี้",
    }
}

impl App {
    /// `{"cmd":"talk","action":"capture"|"cancel"|"preset"|"mode"|"reset","value":…}`
    pub(super) fn talk_message(&mut self, v: &Value) {
        match v["action"].as_str().unwrap_or("") {
            "capture" => self.start_capture(),
            "cancel" => self.cancel_capture(),
            "preset" => {
                let key = v["value"].as_str().and_then(ModKey::from_id);
                if let Some(key) = key.filter(|k| platform::TALK_PRESETS.iter().any(|(p, _)| p == k)) {
                    self.cancel_capture();
                    self.set_talk_key(Binding::Modifier { key });
                }
            }
            "mode" => {
                if let Ok(mode) = serde_json::from_value::<TalkMode>(v["value"].clone()) {
                    self.cfg.talk_mode = mode;
                    self.cfg.save();
                    eprintln!("talk mode: {mode:?}");
                }
            }
            "reset" => {
                self.cancel_capture();
                self.cfg.talk_mode = TalkMode::default();
                self.set_talk_key(Binding::DEFAULT);
            }
            _ => {}
        }
    }

    /// The Settings page's `talk` object.
    pub(super) fn talk_state(&self) -> Value {
        let preset = match self.cfg.talk_key {
            Binding::Modifier { key } if platform::TALK_PRESETS.iter().any(|(p, _)| *p == key) => Value::String(key.id()),
            _ => Value::Null,
        };
        let presets: Vec<Value> = platform::TALK_PRESETS.iter().map(|(k, label)| json!({ "id": k.id(), "label": label })).collect();
        json!({
            "mode": self.cfg.talk_mode,
            "preset": preset,
            "capturing": self.capturing,
            "fn": self.cfg.talk_key == Binding::Modifier { key: ModKey::Fn },
            "presets": presets,
        })
    }

    fn set_talk_key(&mut self, b: Binding) {
        hotkey::lock().set_binding(b);
        self.cfg.talk_key = b;
        self.cfg.save();
        let label = b.label();
        eprintln!("talk key: {label}");
        self.settings_toast(&format!("ตั้งปุ่มพูดเป็น {label} แล้ว"));
    }

    /// The matcher already uses `b` (it switches as soon as the key is pressed).
    pub(super) fn talk_key_captured(&mut self, b: Binding) {
        self.capturing = false;
        self.set_talk_key(b);
    }

    fn start_capture(&mut self) {
        if self.busy() {
            self.settings_toast("รอให้ถอดเสียงเสร็จก่อน");
            return;
        }
        let generation = {
            let mut m = hotkey::lock();
            m.set_capture(true);
            m.capture_gen()
        };
        self.capturing = true;
        eprintln!("recording a new talk key");
        let p = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CAPTURE_TIMEOUT);
            let timed_out = {
                let mut m = hotkey::lock();
                let current = m.capturing() && m.capture_gen() == generation;
                if current {
                    m.set_capture(false);
                }
                current
            };
            if timed_out {
                let _ = p.send_event(UserEvent::Hotkey(HotkeyEvent::CaptureCancelled, Instant::now()));
            }
        });
    }

    pub(super) fn cancel_capture(&mut self) {
        if self.capturing {
            hotkey::lock().set_capture(false);
            self.capturing = false;
        }
    }

    /// Starts hands-free: from a tap (hold-or-tap) or from the first press (tap to start/stop).
    pub(super) fn go_handsfree(&mut self) {
        self.handsfree = true;
        if self.cfg.live_typing
            && let Some(seg) = self.segmenter.as_mut()
        {
            seg.set_live(); // shorter parts, so text appears sooner
        }
        self.status_text = format!("Listening… (tap {} to finish)", hotkey::label());
        self.overlay.set(crate::overlay::Phase::Listening { handsfree: true });
    }

    /// Stops the recording and throws it away (Esc, or the talk key was part of a combination).
    pub(super) fn drop_recording(&mut self) {
        self.stop_at = None;
        self.recorder.stop_raw();
        self.segmenter = None;
        if let Some(p) = self.pipeline.take() {
            p.cancel();
        }
        self.rec_gen += 1;
        self.end_live(); // what was typed already stays
        self.handsfree = false;
        self.state = super::State::Idle;
    }

    pub(super) fn refresh_hint(&self, ui: &Ui) {
        let text = menu_hint(self.cfg.talk_mode);
        if *ui.shown_hint.borrow() != text {
            ui.hint.set_text(&text);
            *ui.shown_hint.borrow_mut() = text;
        }
    }
}

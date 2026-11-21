//! Post-processing of transcribed text: filler words (fillers.rs), the optional language-model
//! clean-up before pasting (polish.rs), and their Settings card.

use std::time::Instant;

use super::{Phase, RECENT, State, UserEvent};
use crate::{corrections, polish};

const TEST_SAMPLE: &str = "วันนี้ เรา มี ประชุม ตอน บ่าย สอง โมง เรื่อง deploy server นะ ครับ";

impl super::App {
    /// What is typed or pasted for transcribed `text`.
    pub(super) fn tidy(&self, text: &str) -> String {
        let text = if self.cfg.remove_fillers { crate::fillers::remove(text) } else { text.to_string() };
        corrections::finish(&text, &self.corrections, self.cfg.numbers_as_digits)
    }

    /// Adds `text` to "Paste last" and Recent.
    pub(super) fn remember(&mut self, text: &str) {
        self.last_text = text.to_string();
        self.recent.push_front(text.to_string());
        self.recent.truncate(RECENT);
        self.recent_dirty = true;
    }

    /// Pastes `text` and ends the recording.
    fn deliver(&mut self, text: String) {
        self.remember(&text);
        let phase = self.paste(text);
        self.overlay.set(phase);
        self.state = State::Idle;
    }

    /// Text to paste at the end of a recording: through the language model first if it is set up.
    pub(super) fn polish_or_deliver(&mut self, text: String) {
        if !self.cfg.polish.ready() {
            return self.deliver(text);
        }
        self.state = State::Transcribing;
        self.polishing = true;
        self.status_text = "Polishing…".into();
        self.overlay.set(Phase::Working);
        let (p, generation, cfg) = (self.proxy.clone(), self.rec_gen, self.cfg.polish.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let r = polish::run(&cfg, &text).and_then(|out| {
                if polish::acceptable(&text, &out) {
                    Ok(out)
                } else {
                    Err(anyhow::anyhow!("reply rejected ({} chars)", out.chars().count()))
                }
            });
            match &r {
                Ok(out) => eprintln!(
                    "polish: {} ms, {} -> {} chars",
                    started.elapsed().as_millis(),
                    text.chars().count(),
                    out.chars().count()
                ),
                Err(e) => eprintln!("polish failed: {e:#}: pasted the original"),
            }
            let _ = p.send_event(UserEvent::Polished(generation, text, r.map_err(|e| format!("{e:#}"))));
        });
    }

    pub(super) fn on_polished(&mut self, generation: u64, original: String, r: Result<String, String>) {
        if generation != self.rec_gen || !self.polishing {
            return; // cancelled with Esc
        }
        self.polishing = false;
        self.status_text = self.ready_text();
        self.deliver(r.unwrap_or(original));
    }

    /// Esc while the language model works: paste nothing.
    pub(super) fn cancel_polish(&mut self) {
        self.rec_gen += 1;
        self.polishing = false;
        self.state = State::Idle;
        self.status_text = "Cancelled".into();
        self.overlay.set(Phase::Cancelled);
        eprintln!("polish cancelled");
    }

    pub(super) fn on_polish_tested(&self, r: Result<u128, String>) {
        match r {
            Ok(ms) => self.settings_toast(&format!("ใช้ได้ ({ms} ms)")),
            Err(e) => self.settings_toast(&format!("ต่อเซิร์ฟเวอร์ไม่ได้: {}", e.chars().take(120).collect::<String>())),
        }
    }

    pub(super) fn post_state(&self) -> serde_json::Value {
        let p = &self.cfg.polish;
        serde_json::json!({
            "fillers": self.cfg.remove_fillers,
            "polish": {
                "enabled": p.enabled,
                "baseUrl": p.base_url,
                "model": p.model,
                "prompt": p.prompt,
                "hasKey": !p.api_key.is_empty(),
                "ready": p.ready(),
            },
        })
    }

    /// `{"cmd":"post","key":"fillers"|"polish","value":bool}`,
    /// `{"cmd":"post","action":"save","baseUrl","model","prompt","key"}` (empty key keeps the stored one),
    /// `{"cmd":"post","action":"test"}`.
    pub(super) fn post_message(&mut self, v: &serde_json::Value) {
        let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        match v["action"].as_str().unwrap_or("") {
            "save" => {
                let prompt = match v["prompt"].as_str().unwrap_or("").trim() {
                    "" => polish::DEFAULT_PROMPT.to_string(),
                    p => p.to_string(),
                };
                if !prompt.contains(polish::PLACEHOLDER) {
                    return self.settings_toast("คำสั่งต้องมี ${output}");
                }
                let p = &mut self.cfg.polish;
                p.base_url = text("baseUrl");
                p.model = text("model");
                p.prompt = prompt;
                if !text("key").is_empty() {
                    p.api_key = text("key");
                }
                self.cfg.save();
                self.settings_toast("บันทึกแล้ว");
            }
            "test" => {
                let (p, cfg) = (self.proxy.clone(), self.cfg.polish.clone());
                std::thread::spawn(move || {
                    let started = Instant::now();
                    let r = polish::run(&cfg, TEST_SAMPLE).and_then(|out| {
                        if polish::acceptable(TEST_SAMPLE, &out) {
                            Ok(started.elapsed().as_millis())
                        } else {
                            Err(anyhow::anyhow!("คำตอบว่างหรือยาวเกินไป ({} ตัวอักษร)", out.chars().count()))
                        }
                    });
                    if let Err(e) = &r {
                        eprintln!("polish test failed: {e:#}");
                    }
                    let _ = p.send_event(UserEvent::PolishTested(r.map_err(|e| format!("{e:#}"))));
                });
            }
            _ => {
                let on = v["value"].as_bool().unwrap_or(false);
                match v["key"].as_str().unwrap_or("") {
                    "fillers" => self.cfg.remove_fillers = on,
                    "polish" => self.cfg.polish.enabled = on,
                    _ => return,
                }
                self.cfg.save();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn fillers_default_on() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.remove_fillers);
        let c: Config = serde_json::from_str(r#"{"remove_fillers": false, "mixed_english": false}"#).unwrap();
        assert!(!c.remove_fillers && !c.mixed_english);
    }
}

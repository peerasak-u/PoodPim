//! Post-processing of transcribed text: filler words (fillers.rs) and the Settings card for it.

use crate::corrections;

impl super::App {
    /// What is typed or pasted for transcribed `text`.
    pub(super) fn tidy(&self, text: &str) -> String {
        let text = if self.cfg.remove_fillers { crate::fillers::remove(text) } else { text.to_string() };
        corrections::finish(&text, &self.corrections, self.cfg.numbers_as_digits)
    }

    pub(super) fn post_state(&self) -> serde_json::Value {
        serde_json::json!({ "fillers": self.cfg.remove_fillers })
    }

    /// `{"cmd":"post","key":"fillers","value":bool}`
    pub(super) fn post_message(&mut self, v: &serde_json::Value) {
        let on = v["value"].as_bool().unwrap_or(false);
        match v["key"].as_str().unwrap_or("") {
            "fillers" => self.cfg.remove_fillers = on,
            _ => return,
        }
        self.cfg.save();
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

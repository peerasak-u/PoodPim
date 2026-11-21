//! Settings › ไมโครโฟน: which input device to record from.

use crate::audio::{self, Recorder};

impl super::App {
    /// `"mic"` in the Settings state.
    pub(super) fn mic_state(&self) -> serde_json::Value {
        let selected = self.cfg.microphone.as_deref();
        let devices = self.mics.as_deref().unwrap_or_default();
        let missing = self.mics.is_some() && selected.is_some() && audio::choose(devices, selected).is_none();
        serde_json::json!({ "selected": selected, "devices": devices, "missing": missing })
    }

    /// `{"cmd":"mic","action":"refresh"}` lists the devices again; `{"cmd":"mic","value":null|"name"}`
    /// picks one (null = the system default) and warms it up.
    pub(super) fn mic_message(&mut self, v: &serde_json::Value) {
        if v["action"] == "refresh" {
            self.mics = Some(audio::input_names());
            return;
        }
        let Some(value) = v.get("value") else { return };
        let wanted = value.as_str().map(str::to_string);
        if wanted == self.cfg.microphone {
            return;
        }
        self.cfg.microphone = wanted;
        self.cfg.save();
        Recorder::warm_up(self.cfg.microphone.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn config_without_microphone_uses_default() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.microphone, None);
    }

    #[test]
    fn config_microphone_round_trips_unicode_name() {
        let c = Config { microphone: Some("ไมค์ USB “Blue Yeti” 🎙".into()), ..Config::default() };
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.microphone, c.microphone);
    }

    #[test]
    fn config_bad_microphone_keeps_other_settings() {
        let c: Config = serde_json::from_str(r#"{"microphone": 5, "mixed_english": false}"#).unwrap();
        assert_eq!(c.microphone, None);
        assert!(!c.mixed_english);
    }

    #[test]
    fn config_null_microphone_is_default() {
        let c: Config = serde_json::from_str(r#"{"microphone": null}"#).unwrap();
        assert_eq!(c.microphone, None);
    }
}

//! Settings › ไมโครโฟน: the microphones to record from, by preference.

use crate::audio::{self, Recorder};

impl super::App {
    /// `"mic"` in the Settings state: the preference list as rows, the connected ones marked,
    /// and the one a recording would use now.
    pub(super) fn mic_state(&self) -> serde_json::Value {
        let connected = self.mics.as_deref();
        let in_use = connected.and_then(|names| audio::choose(names, &self.cfg.microphones)).map(|i| connected.unwrap()[i].clone());
        let rows: Vec<_> = self
            .cfg
            .microphones
            .iter()
            .map(|m| match m {
                None => serde_json::json!({ "name": null, "connected": true, "inUse": connected.is_some() && in_use.is_none() }),
                Some(name) => serde_json::json!({
                    "name": name,
                    "connected": connected.is_none_or(|c| c.contains(name)),
                    "inUse": in_use.as_deref() == Some(name.as_str()),
                }),
            })
            .collect();
        serde_json::json!({ "rows": rows, "listed": connected.is_some(), "defaultName": self.default_mic })
    }

    /// `{"cmd":"mic","action":"refresh"}` lists the connected microphones (and remembers new ones
    /// at the end of the list); `{"cmd":"mic","order":[name|null, …]}` saves a new order and warms
    /// up the microphone it picks.
    pub(super) fn mic_message(&mut self, v: &serde_json::Value) {
        if v["action"] == "refresh" {
            let names = audio::input_names();
            self.default_mic = audio::default_input_name();
            if audio::remember_mics(&mut self.cfg.microphones, &names) {
                self.cfg.save();
            }
            self.mics = Some(names);
            return;
        }
        let Ok(mut order) = audio::de_mic_list(&v["order"]) else { return };
        audio::tidy_mics(&mut order);
        if order == self.cfg.microphones {
            return;
        }
        self.cfg.microphones = order;
        self.cfg.save();
        Recorder::warm_up(self.cfg.microphones.clone());
    }
}

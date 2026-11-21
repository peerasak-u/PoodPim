use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::hotkey::{Binding, TalkMode};
use crate::remote::Server;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Run the model on this computer.
    #[default]
    Local,
    /// Send audio to the Lambda Function URL in `server_url`.
    Lambda,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub backend: Backend,
    /// Microphones by preference: each recording uses the first one connected (see
    /// audio::choose). None is the system default, which is always there, so the ones below it
    /// are remembered but unused until moved above it.
    #[serde(deserialize_with = "crate::audio::de_mic_list")]
    pub microphones: Vec<Option<String>>,
    /// The single microphone of older versions (None = the system default). Read once into
    /// `microphones`, never written.
    #[serde(deserialize_with = "crate::audio::de_device_name", skip_serializing)]
    microphone: Option<String>,
    /// Lambda Function URL, e.g. https://xxxx.lambda-url.ap-southeast-7.on.aws/
    pub server_url: String,
    pub api_key: String,
    /// Thai + English prompt (63) instead of Thai only (32).
    pub mixed_english: bool,
    /// The key held (or tapped) to talk, see hotkey.rs. Windows always uses Right Alt.
    pub talk_key: Binding,
    /// The key that cancels a dictation (macOS; Windows always uses Esc).
    #[serde(deserialize_with = "lenient")]
    pub cancel_key: crate::hotkey::Chord,
    /// Hold or tap (stopping on silence) / hold only / tap to start and stop.
    pub talk_mode: TalkMode,
    /// `TalkMode::Auto`, hands-free: seconds of silence after speech that finish the recording.
    pub auto_stop_secs: f32,
    /// Put the previous clipboard back after pasting.
    pub restore_clipboard: bool,
    /// Start the app when the user logs in (macOS LaunchAgent / Windows Run key).
    pub start_at_login: bool,
    /// Write numbers said in words as digits ("สองร้อยห้าสิบ" → "250"), see numbers.rs.
    pub numbers_as_digits: bool,
    /// Free the local model's memory (~0.75 GB) after a while without dictation; it loads
    /// again, while the user speaks, on the next press of the talk key.
    pub unload_when_idle: bool,
    /// With `unload_when_idle`: minutes without dictation before the model is freed (0: right
    /// after each dictation).
    pub unload_after_min: u32,
    /// Hands-free mode (tap the talk key): type each part as soon as it is transcribed, while
    /// the user is still speaking, instead of all of it at the end.
    pub live_typing: bool,
    /// While listening, the pill shows what has been heard so far (local model only: it
    /// transcribes the unfinished part again about every 0.7 s).
    pub live_preview: bool,
    /// Said "later" to replacing the old full-precision model with the smaller int8 one.
    pub model_upgrade_declined: bool,
    /// Drop filler words ("เอ่อ", "อืม", "um") before typing or pasting, see fillers.rs.
    pub remove_fillers: bool,
    /// Where on the screen the dictation pill appears.
    pub pill_position: crate::overlay::Position,
    /// Clean up the final text with a language model before pasting, see polish.rs.
    #[serde(deserialize_with = "lenient")]
    pub polish: Polish,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: Backend::Local,
            microphones: vec![None],
            microphone: None,
            server_url: String::new(),
            api_key: String::new(),
            mixed_english: true,
            talk_key: Binding::DEFAULT,
            cancel_key: Default::default(),
            talk_mode: TalkMode::Auto,
            auto_stop_secs: DEFAULT_AUTO_STOP_SECS,
            restore_clipboard: true,
            start_at_login: true,
            numbers_as_digits: true,
            unload_when_idle: false,
            unload_after_min: 15,
            live_typing: true,
            live_preview: true,
            model_upgrade_declined: false,
            remove_fillers: true,
            pill_position: Default::default(),
            polish: Polish::default(),
        }
    }
}

/// `auto_stop_secs` unless set: long enough for a Thai speaker's thinking pause (the
/// segmenter treats 1.2 s as a long pause), short enough not to feel stuck.
pub const DEFAULT_AUTO_STOP_SECS: f32 = 2.0;

/// The longest wait `unload_after_min` can set.
const MAX_UNLOAD_MIN: u32 = 24 * 60;

/// An OpenAI-compatible chat endpoint that cleans up the text pasted at the end of a recording.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Polish {
    pub enabled: bool,
    /// e.g. http://127.0.0.1:4994/v1 (`/chat/completions` is added).
    pub base_url: String,
    /// Optional for local servers.
    pub api_key: String,
    pub model: String,
    /// Sent as the one user message, with `${output}` replaced by the text.
    pub prompt: String,
    pub timeout_s: u32,
}

impl Default for Polish {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            prompt: crate::polish::DEFAULT_PROMPT.to_string(),
            timeout_s: 20,
        }
    }
}

impl Polish {
    pub fn ready(&self) -> bool {
        self.enabled && !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// A malformed value falls back to the default instead of failing the whole file.
fn lenient<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(v).unwrap_or_default())
}

pub fn path() -> PathBuf {
    crate::model::app_dir(dirs::config_dir()).join("config.json")
}

fn read(p: &std::path::Path) -> Option<Config> {
    parse(&std::fs::read_to_string(p).ok()?)
}

/// A config file's text, with an older version's single microphone moved to the top of the list.
/// None only if it isn't a JSON object. A setting this version can't read (a value from a newer
/// or an older version, or edited by hand) keeps its default; the others are kept.
fn parse(text: &str) -> Option<Config> {
    let serde_json::Value::Object(file) = serde_json::from_str(text).ok()? else { return None };
    let mut c: Config = match serde_json::from_value(serde_json::Value::Object(file.clone())) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e}; keeping the settings that can be read");
            let mut kept = serde_json::Map::new();
            for (key, value) in file {
                kept.insert(key.clone(), value);
                if serde_json::from_value::<Config>(serde_json::Value::Object(kept.clone())).is_err() {
                    eprintln!("config: `{key}` reset to its default");
                    kept.remove(&key);
                }
            }
            serde_json::from_value(serde_json::Value::Object(kept)).ok()?
        }
    };
    if let Some(name) = c.microphone.take() {
        c.microphones.insert(0, Some(name));
    }
    crate::audio::tidy_mics(&mut c.microphones);
    Some(c)
}

/// Settings bundled for people the app is shared with (see scripts/package-macos.sh):
/// PoodPim.app/Contents/Resources/preset.json on macOS, preset.json next to the .exe on Windows.
fn preset_path() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    if cfg!(target_os = "macos") {
        Some(exe_dir.parent()?.join("Resources").join("preset.json"))
    } else {
        Some(exe_dir.join("preset.json"))
    }
}

impl Config {
    /// Loads the user's config. On first run (no config file yet) it starts from the
    /// `preset.json` shipped with the app, if any, and saves that as the user's config. A file
    /// that isn't JSON at all is kept as config.json.bad before anything overwrites it.
    pub fn load() -> Self {
        let p = path();
        if let Some(c) = read(&p) {
            return c;
        }
        if p.exists() {
            eprintln!("config: {} can't be read; kept as config.json.bad", p.display());
            let _ = std::fs::rename(&p, p.with_extension("json.bad"));
        }
        match preset_path().as_deref().and_then(read) {
            Some(c) => {
                c.save();
                c
            }
            None => Self::default(),
        }
    }

    pub fn save(&self) {
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let Ok(s) = serde_json::to_string_pretty(self) else { return };
        // Written beside it and renamed over it, so a crash mid-write can't leave half a file.
        let tmp = p.with_extension("json.tmp");
        let written = (|| -> std::io::Result<()> {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            // The file holds the API key: private to this user from the start.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            std::io::Write::write_all(&mut o.open(&tmp)?, s.as_bytes())?;
            std::fs::rename(&tmp, &p)
        })();
        if let Err(e) = written {
            eprintln!("config: saving {}: {e}", p.display());
        }
    }

    /// Minutes without dictation before the local model is freed (None: never).
    pub fn unload_min(&self) -> Option<u32> {
        self.unload_when_idle.then_some(self.unload_after_min.min(MAX_UNLOAD_MIN))
    }

    pub fn unload_after(&self) -> Option<Duration> {
        self.unload_min().map(|m| Duration::from_secs(u64::from(m) * 60))
    }

    /// From Settings: minutes before the idle model is freed, or None to never free it (the
    /// minutes are kept for next time).
    pub fn set_unload(&mut self, min: Option<u64>) {
        self.unload_when_idle = min.is_some();
        if let Some(m) = min {
            self.unload_after_min = m.min(u64::from(MAX_UNLOAD_MIN)) as u32;
        }
    }

    pub fn server(&self) -> Server {
        Server { url: self.server_url.trim().to_string(), api_key: self.api_key.trim().to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Config {
        serde_json::from_str(json).unwrap()
    }

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn unload_off_by_default() {
        assert_eq!(parse("{}").unload_after(), None);
    }

    #[test]
    fn old_config_with_unload_on_keeps_15_min() {
        assert_eq!(parse(r#"{"unload_when_idle":true}"#).unload_after(), Some(15 * MIN));
    }

    #[test]
    fn unload_minutes_are_read_and_clamped() {
        assert_eq!(parse(r#"{"unload_when_idle":true,"unload_after_min":5}"#).unload_after(), Some(5 * MIN));
        assert_eq!(parse(r#"{"unload_when_idle":true,"unload_after_min":0}"#).unload_after(), Some(Duration::ZERO));
        let c = parse(r#"{"unload_when_idle":true,"unload_after_min":99999}"#);
        assert_eq!(c.unload_min(), Some(24 * 60));
        assert_eq!(c.unload_after(), Some(24 * 60 * MIN));
    }

    #[test]
    fn unload_off_ignores_minutes() {
        assert_eq!(parse(r#"{"unload_when_idle":false,"unload_after_min":5}"#).unload_after(), None);
    }

    #[test]
    fn set_unload_from_settings() {
        let mut c = Config::default();
        c.set_unload(Some(5));
        assert_eq!(c.unload_min(), Some(5));
        c.set_unload(Some(0));
        assert_eq!(c.unload_min(), Some(0));
        c.set_unload(Some(99999));
        assert_eq!(c.unload_after_min, 24 * 60);
        c.set_unload(Some(30));
        c.set_unload(None);
        assert_eq!(c.unload_min(), None);
        c.unload_when_idle = true;
        assert_eq!(c.unload_min(), Some(30));
    }

    #[test]
    fn a_setting_this_version_cant_read_keeps_the_rest() {
        let c = super::parse(
            r#"{"server_url":"https://x.example","api_key":"k","talk_mode":"from_the_future","talk_key":{"type":"modifier","key":"right_command"},"mixed_english":false}"#,
        )
        .unwrap();
        assert_eq!((c.server_url.as_str(), c.api_key.as_str()), ("https://x.example", "k"));
        assert!(!c.mixed_english);
        assert_eq!(c.talk_mode, TalkMode::default());
        assert_eq!(c.talk_key, Binding::DEFAULT);
        assert!(super::parse("not json").is_none());
        assert!(super::parse("[1]").is_none());
    }

    #[test]
    fn microphone_list_defaults_and_migrates() {
        let list = |text: &str| super::parse(text).unwrap().microphones;
        assert_eq!(list("{}"), vec![None]);
        // An older version's single choice moves to the top; the system default follows.
        assert_eq!(list(r#"{"microphone": "HyperX SoloCast"}"#), vec![Some("HyperX SoloCast".into()), None]);
        assert_eq!(list(r#"{"microphone": null}"#), vec![None]);
        let c = super::parse(r#"{"microphones": ["ไมค์ USB “Blue Yeti” 🎙", 5, null], "mixed_english": false}"#).unwrap();
        assert_eq!(c.microphones, vec![Some("ไมค์ USB “Blue Yeti” 🎙".into()), None]);
        assert!(!c.mixed_english);
        // Written back as the list only.
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains(r#""microphones":["ไมค์ USB “Blue Yeti” 🎙",null]"#) && !text.contains(r#""microphone":"#));
    }
}

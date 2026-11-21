//! Settings › คำศัพท์: the vocabulary and corrections files, edited as categories (see lists.rs).

use crate::lists::{self, Category, Kind};
use crate::{corrections, hotwords};

impl super::App {
    /// Both lists for the page: `{"vocabulary": [categories], "corrections": [categories]}`.
    fn lists_json(&self) -> String {
        let (vocabulary, _) = hotwords::read_user_list();
        let (corrections, _) = corrections::read_user_list();
        serde_json::json!({
            "vocabulary": lists::parse(&vocabulary).categories,
            "corrections": lists::parse(&corrections).categories,
        })
        .to_string()
    }

    /// Sends both lists to the Settings page, if it is open.
    pub(super) fn push_lists(&self) {
        if let Some(w) = &self.settings {
            w.show_lists(&self.lists_json());
        }
    }

    /// `{"cmd":"lists","action":"get"}` sends the lists (again: the files may have been edited
    /// elsewhere); `{"cmd":"lists","action":"save","list":"vocabulary"|"corrections","categories":[…]}`
    /// writes one back, keeping the file's preamble.
    pub(super) fn lists_message(&mut self, v: &serde_json::Value) {
        if v["action"] == "save" {
            let (kind, path, text) = match v["list"].as_str() {
                Some("vocabulary") => (Kind::Vocabulary, hotwords::user_file(), hotwords::read_user_list().0),
                Some("corrections") => (Kind::Corrections, corrections::user_file(), corrections::read_user_list().0),
                _ => return,
            };
            let Ok(categories) = serde_json::from_value::<Vec<Category>>(v["categories"].clone()) else { return };
            let doc = lists::Doc { preamble: lists::parse(&text).preamble, categories: lists::tidy(kind, categories) };
            if let Err(e) = std::fs::write(&path, lists::format(&doc)) {
                self.settings_toast(&format!("บันทึกไม่ได้: {e}"));
            }
        }
        self.reload_vocabulary();
        self.push_lists();
    }
}

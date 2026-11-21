//! Drops filler words ("เอ่อ", "อืม", "um", "uh") from transcribed text.
//!
//! Conservative on purpose: Thai fillers that also start real words ("อ่า" in "อ่าน") go only
//! when they stand alone, and English ones only as whole words ("um" but not "summer").

/// English fillers, matched as whole words in any case.
const ENGLISH: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "er", "erm", "hm", "hmm", "mmm"];
/// Thai fillers removed only between spaces, punctuation or the ends of the text.
const THAI_STANDALONE: &[&str] = &["อ่า", "เออ", "อ่อ"];

pub fn remove(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(n) = filler_at(&chars, i) {
            i += n;
            if chars.get(i) == Some(&',') {
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    collapse_spaces(&out)
}

/// Length in chars of the filler starting at `i`, if there is one.
fn filler_at(chars: &[char], i: usize) -> Option<usize> {
    let c = chars[i];
    if c.is_ascii_alphabetic() {
        if i > 0 && chars[i - 1].is_ascii_alphanumeric() {
            return None;
        }
        let end = (i..chars.len()).find(|&j| !chars[j].is_ascii_alphanumeric()).unwrap_or(chars.len());
        let word: String = chars[i..end].iter().collect::<String>().to_ascii_lowercase();
        return ENGLISH.contains(&word.as_str()).then_some(end - i);
    }
    if starts(chars, i, "เอิ่ม") {
        return Some(5);
    }
    if starts(chars, i, "อืม") {
        let extra = chars[i + 3..].iter().take_while(|&&c| c == 'ม').count();
        return Some(3 + extra);
    }
    if starts(chars, i, "เอ่อ") && !starts(chars, i + 4, "ล้น") {
        return Some(4);
    }
    THAI_STANDALONE.iter().find_map(|w| {
        let n = w.chars().count();
        let before = i == 0 || is_break(chars[i - 1]);
        let after = chars.get(i + n).is_none_or(|&c| is_break(c));
        (starts(chars, i, w) && before && after).then_some(n)
    })
}

fn starts(chars: &[char], i: usize, word: &str) -> bool {
    let mut k = i;
    for w in word.chars() {
        if chars.get(k) != Some(&w) {
            return false;
        }
        k += 1;
    }
    true
}

fn is_break(c: char) -> bool {
    c.is_whitespace() || c.is_ascii_punctuation() || c == '…'
}

fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::remove;

    #[test]
    fn english_fillers_go_as_whole_words() {
        assert_eq!(remove("um, deploy uh server"), "deploy server");
        assert_eq!(remove("Um so UH yes"), "so yes");
        assert_eq!(remove("summer number umbrella hummus"), "summer number umbrella hummus");
        assert_eq!(remove("error err erm"), "error err");
    }

    #[test]
    fn thai_fillers_go_anywhere() {
        assert_eq!(remove("เอ่อ วันนี้ อืม ประชุม"), "วันนี้ ประชุม");
        assert_eq!(remove("วันนี้อืมมมประชุม"), "วันนี้ประชุม");
        assert_eq!(remove("เอิ่ม ไม่รู้"), "ไม่รู้");
    }

    #[test]
    fn real_words_stay() {
        for s in ["อ่านหนังสือ", "อ่างน้ำ", "เอ่อล้น", "summer", "number", "อ่อนแอ", "เออร์เบิน"] {
            assert_eq!(remove(s), s);
        }
    }

    #[test]
    fn standalone_thai_fillers() {
        assert_eq!(remove("อ่า"), "");
        assert_eq!(remove("อ่า ไปกัน"), "ไปกัน");
        assert_eq!(remove("ใช่ เออ ได้"), "ใช่ ได้");
        assert_eq!(remove("อ่อ, เข้าใจแล้ว"), "เข้าใจแล้ว");
    }

    #[test]
    fn empty_and_filler_only() {
        assert_eq!(remove(""), "");
        assert_eq!(remove("  um uh  "), "");
    }
}

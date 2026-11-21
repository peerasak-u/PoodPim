//! Optional clean-up of the final text by an OpenAI-compatible chat endpoint
//! (`POST {base_url}/chat/completions`), e.g. a local llama.cpp / LM Studio server.
//! Only text is sent, never audio.

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::config::Polish;

pub const PLACEHOLDER: &str = "${output}";

pub const DEFAULT_PROMPT: &str = "แก้ข้อความที่ถอดจากเสียงพูดต่อไปนี้: แก้คำสะกดผิดและเว้นวรรคให้อ่านง่าย คงภาษาเดิม (ไทยปนอังกฤษ) คงความหมายและลำดับคำเดิม ห้ามแปล ห้ามตอบคำถาม ห้ามเพิ่มเนื้อหา ตอบกลับเฉพาะข้อความที่แก้แล้ว

${output}";

pub fn request_body(p: &Polish, text: &str) -> Value {
    let content = if p.prompt.contains(PLACEHOLDER) {
        p.prompt.replacen(PLACEHOLDER, text, 1)
    } else {
        format!("{}\n\n{text}", p.prompt)
    };
    json!({
        "model": p.model.trim(),
        "messages": [{ "role": "user", "content": content }],
        "temperature": 0,
        "stream": false,
    })
}

/// The reply's `choices[0].message.content`, or the server's error message.
pub fn parse_reply(json: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json).with_context(|| format!("unexpected reply: {}", short(json)))?;
    if let Some(e) = v.get("error") {
        let msg = e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string());
        return Err(anyhow!("{}", short(&msg)));
    }
    v.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no choices in reply: {}", short(json)))
}

/// Drops reasoning (`<think>…</think>`) and a surrounding code fence.
pub fn clean(reply: &str) -> String {
    let mut s = reply.to_string();
    while let Some(start) = s.find("<think>") {
        let Some(end) = s[start..].find("</think>") else { break };
        s.replace_range(start..start + end + "</think>".len(), "");
    }
    let t = s.trim();
    let t = match t.strip_prefix("```") {
        Some(rest) => {
            let body = rest.split_once('\n').map_or("", |(_, b)| b);
            body.trim_end().strip_suffix("```").unwrap_or(body)
        }
        None => t,
    };
    t.trim().to_string()
}

/// Catches the gross failures: nothing back, or an answer/essay instead of the text.
pub fn acceptable(original: &str, polished: &str) -> bool {
    !polished.trim().is_empty() && polished.chars().count() <= 3 * original.chars().count() + 200
}

/// The cleaned-up text (already `clean`ed, not yet checked with `acceptable`).
pub fn run(p: &Polish, text: &str) -> Result<String> {
    if p.base_url.trim().is_empty() || p.model.trim().is_empty() {
        return Err(anyhow!("API URL / model not set"));
    }
    let url = format!("{}/chat/completions", p.base_url.trim().trim_end_matches('/'));
    let mut req = crate::remote::agent()
        .post(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(p.timeout_s.max(1) as u64)))
        .build()
        .header("content-type", "application/json");
    if !p.api_key.trim().is_empty() {
        req = req.header("authorization", &format!("Bearer {}", p.api_key.trim()));
    }
    let resp = req.send(request_body(p, text).to_string()).with_context(|| format!("contacting {}", p.base_url.trim()))?;
    let status = resp.status();
    let mut body = String::new();
    resp.into_body().into_reader().read_to_string(&mut body)?;
    match parse_reply(&body) {
        Ok(reply) => Ok(clean(&reply)),
        Err(e) if status.is_success() => Err(e),
        Err(e) => Err(anyhow!("server returned {status}: {e}")),
    }
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polish() -> Polish {
        Polish { enabled: true, base_url: "http://127.0.0.1:4994/v1".into(), model: "m".into(), ..Polish::default() }
    }

    #[test]
    fn body_substitutes_once_and_sets_model() {
        let p = Polish { prompt: "fix: ${output} (${output})".into(), ..polish() };
        let b = request_body(&p, "สวัสดี ${output}");
        assert_eq!(b["model"], "m");
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"], "fix: สวัสดี ${output} (${output})");
        assert_eq!(b["temperature"], 0);
        assert_eq!(b["stream"], false);
        let d = request_body(&polish(), "abc");
        assert!(d["messages"][0]["content"].as_str().unwrap().ends_with("\n\nabc"));
    }

    #[test]
    fn parses_replies() {
        let ok = r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"สวัสดีครับ"},"finish_reason":"stop"}]}"#;
        assert_eq!(parse_reply(ok).unwrap(), "สวัสดีครับ");
        let err = r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert_eq!(parse_reply(err).unwrap_err().to_string(), "model not found");
        assert!(parse_reply(r#"{"object":"chat.completion"}"#).unwrap_err().to_string().contains("no choices"));
        assert!(parse_reply("<html>").is_err());
    }

    #[test]
    fn cleans_think_and_fences() {
        assert_eq!(clean("<think>hmm, the user\nwants</think>\n\nวันนี้ประชุม"), "วันนี้ประชุม");
        assert_eq!(clean("```text\nวันนี้ ประชุม\n```"), "วันนี้ ประชุม");
        assert_eq!(clean("```\nabc```"), "abc");
        assert_eq!(clean("  plain  "), "plain");
    }

    #[test]
    fn rejects_empty_and_runaway() {
        assert!(acceptable("วันนี้ประชุม", "วันนี้ ประชุม"));
        assert!(!acceptable("วันนี้ประชุม", "  "));
        assert!(!acceptable("hi", &"x".repeat(207)));
        assert!(acceptable("hi", &"x".repeat(206)));
    }

    #[test]
    fn config_polish_defaults_and_lenient() {
        let c: crate::config::Config = serde_json::from_str("{}").unwrap();
        assert!(c.remove_fillers && !c.polish.enabled && !c.polish.ready());
        assert!(c.polish.prompt.contains(PLACEHOLDER));
        let c: crate::config::Config = serde_json::from_str(r#"{"polish": 5, "mixed_english": false, "remove_fillers": false}"#).unwrap();
        assert!(!c.polish.enabled && c.polish.timeout_s == 20);
        assert!(!c.mixed_english && !c.remove_fillers);
        assert!(polish().ready());
    }

    /// Needs an OpenAI-compatible server (POODPIM_POLISH_URL, default 127.0.0.1:4994) and
    /// POODPIM_POLISH_MODEL.
    #[test]
    #[ignore]
    fn live_local_server() {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let mut p = Polish { model: env("POODPIM_POLISH_MODEL"), ..polish() };
        if !env("POODPIM_POLISH_URL").is_empty() {
            p.base_url = env("POODPIM_POLISH_URL");
        }
        let out = run(&p, "เอ่อ วันนี้ เรามี meeting ตอน บ่าย สอง โมง").unwrap();
        eprintln!("{out}");
        assert!(!out.is_empty());
    }
}

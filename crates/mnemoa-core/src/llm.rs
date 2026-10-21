//! HTTP drafter for the optional LLM feature (`--features llm`, `llm.enabled = true`).
//!
//! Providers:
//! - `ollama`, `openai`, `custom`: OpenAI-compatible `POST {base_url}/chat/completions`.
//! - `anthropic`: the Messages API, `POST {base_url}/v1/messages` (default base URL
//!   `https://api.anthropic.com`), with the server-side refusal fallback enabled.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::LlmConfig;
use crate::drafter::{
    Draft, DraftError, DraftRequest, Drafter, SYSTEM_PROMPT, parse_draft, user_prompt,
};

/// Default Anthropic model when the configured model is still the Ollama default.
pub const ANTHROPIC_DEFAULT_MODEL: &str = "claude-opus-5-5";
const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const OLLAMA_BASE: &str = "http://localhost:11434/v1";
const OLLAMA_MODEL: &str = "qwen2.5-coder:7b";

/// Drafter backed by an HTTP model endpoint.
pub struct HttpDrafter {
    cfg: LlmConfig,
    client: reqwest::blocking::Client,
}

impl HttpDrafter {
    /// Build a drafter. Fails with [`DraftError::Disabled`] unless `llm.enabled` is set.
    pub fn new(cfg: &LlmConfig) -> Result<Self, DraftError> {
        if !cfg.enabled {
            return Err(DraftError::Disabled);
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| DraftError::Request(e.to_string()))?;
        Ok(HttpDrafter {
            cfg: cfg.clone(),
            client,
        })
    }

    fn anthropic(&self, req: &DraftRequest) -> Result<String, DraftError> {
        let base = if self.cfg.base_url == OLLAMA_BASE || self.cfg.base_url.is_empty() {
            ANTHROPIC_BASE.to_string()
        } else {
            self.cfg.base_url.trim_end_matches('/').to_string()
        };
        let model = if self.cfg.model == OLLAMA_MODEL || self.cfg.model.is_empty() {
            ANTHROPIC_DEFAULT_MODEL.to_string()
        } else {
            self.cfg.model.clone()
        };
        let body = json!({
            "model": model,
            "max_tokens": 16000,
            "system": SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": user_prompt(req)}],
            "fallbacks": "default",
        });
        let resp = self
            .client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .json(&body)
            .send()
            .map_err(|e| DraftError::Request(e.to_string()))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| DraftError::Request(e.to_string()))?;
        if !status.is_success() {
            return Err(DraftError::Request(format!(
                "HTTP {status}: {}",
                v["error"]["message"].as_str().unwrap_or("unknown error")
            )));
        }
        anthropic_text(&v)
    }

    fn openai_compatible(&self, req: &DraftRequest) -> Result<String, DraftError> {
        let body = json!({
            "model": self.cfg.model,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_prompt(req)},
            ],
        });
        let resp = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.cfg.base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.cfg.api_key)
            .json(&body)
            .send()
            .map_err(|e| DraftError::Request(e.to_string()))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| DraftError::Request(e.to_string()))?;
        if !status.is_success() {
            return Err(DraftError::Request(format!(
                "HTTP {status}: {}",
                v["error"]["message"].as_str().unwrap_or("unknown error")
            )));
        }
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| DraftError::Parse("no choices[0].message.content in reply".into()))
    }
}

/// Text from a Messages API response, checking the stop reason first.
pub fn anthropic_text(v: &Value) -> Result<String, DraftError> {
    if v["stop_reason"] == "refusal" {
        let category = v["stop_details"]["category"]
            .as_str()
            .unwrap_or("unspecified");
        return Err(DraftError::Refused(format!("refusal ({category})")));
    }
    let text: Vec<&str> = v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect()
        })
        .unwrap_or_default();
    if text.is_empty() {
        return Err(DraftError::Parse("no text blocks in reply".into()));
    }
    Ok(text.join(""))
}

impl Drafter for HttpDrafter {
    fn draft(&self, req: &DraftRequest) -> Result<Draft, DraftError> {
        let text = match self.cfg.provider.as_str() {
            "anthropic" => self.anthropic(req)?,
            "ollama" | "openai" | "custom" => self.openai_compatible(req)?,
            other => return Err(DraftError::Request(format!("unknown provider `{other}`"))),
        };
        parse_draft(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve one canned JSON response and return the request text it received.
    fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let mut req = String::new();
            loop {
                let n = stream.read(&mut buf).unwrap();
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if let Some(i) = req.find("\r\n\r\n") {
                    let len: usize = req
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if req.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            req
        });
        (addr, handle)
    }

    fn cfg(provider: &str, base: &str, model: &str) -> LlmConfig {
        LlmConfig {
            enabled: true,
            provider: provider.into(),
            base_url: base.into(),
            model: model.into(),
            api_key: "k".into(),
        }
    }

    fn req() -> DraftRequest {
        DraftRequest {
            rule: "Never use eval".into(),
            language: "python".into(),
            scope: vec![],
        }
    }

    #[test]
    fn disabled_config_refuses() {
        let mut c = cfg("ollama", OLLAMA_BASE, OLLAMA_MODEL);
        c.enabled = false;
        assert!(matches!(HttpDrafter::new(&c), Err(DraftError::Disabled)));
    }

    #[test]
    fn openai_compatible_round_trip() {
        let (addr, h) = serve_once(
            r#"{"choices":[{"message":{"content":"{\"rule\":\"pattern: eval($$$)\",\"positive\":[\"eval(x)\"],\"negative\":[\"f(x)\"]}"}}]}"#,
        );
        let d = HttpDrafter::new(&cfg("ollama", &format!("{addr}/v1"), "m"))
            .unwrap()
            .draft(&req())
            .unwrap();
        assert_eq!(d.rule, "pattern: eval($$$)");
        let sent = h.join().unwrap();
        assert!(sent.starts_with("POST /v1/chat/completions"));
        assert!(sent.to_lowercase().contains("authorization: bearer k"));
    }

    #[test]
    fn anthropic_round_trip_uses_messages_api() {
        let (addr, h) = serve_once(
            r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"{\"rule\":\"pattern: eval($$$)\",\"positive\":[\"eval(x)\"],\"negative\":[\"f(x)\"]}"}]}"#,
        );
        let d = HttpDrafter::new(&cfg("anthropic", &addr, OLLAMA_MODEL))
            .unwrap()
            .draft(&req())
            .unwrap();
        assert_eq!(d.negative, vec!["f(x)"]);
        let sent = h.join().unwrap();
        let lower = sent.to_lowercase();
        assert!(sent.starts_with("POST /v1/messages"));
        assert!(lower.contains("x-api-key: k") && lower.contains("anthropic-version: 2023-06-01"));
        assert!(sent.contains(ANTHROPIC_DEFAULT_MODEL));
        assert!(!sent.contains("temperature"));
    }

    #[test]
    fn anthropic_refusal_is_reported() {
        let v: Value = serde_json::from_str(
            r#"{"stop_reason":"refusal","stop_details":{"category":"cyber"},"content":[]}"#,
        )
        .unwrap();
        assert!(matches!(anthropic_text(&v), Err(DraftError::Refused(m)) if m.contains("cyber")));
    }
}

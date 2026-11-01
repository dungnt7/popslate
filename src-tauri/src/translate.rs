use serde::Deserialize;
use std::time::Duration;

// Free endpoint used by the Google Translate browser extensions. Unofficial: no contract.
// Measured 2026-11-01: translate.google.com answers in ~45 ms, translate.googleapis.com in ~430 ms
// (same JSON, same model). A single POST handles 58k chars (~5 s), so no chunking.
const URL: &str = "https://translate.google.com/translate_a/single";

#[derive(Deserialize)]
struct Resp {
    sentences: Vec<Sentence>,
    src: String,
}

#[derive(Deserialize)]
struct Sentence {
    #[serde(default)]
    trans: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Translation {
    pub text: String,
    /// Detected source language, e.g. "en".
    pub src: String,
    /// Language actually translated to (target, or fallback when source == target).
    pub tl: String,
}

/// Cheap to clone: reqwest::Client shares one connection pool.
#[derive(Clone)]
pub struct Translator {
    client: reqwest::Client,
}

impl Translator {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            // Long texts (~60k chars) take ~5 s server-side.
            .timeout(Duration::from_secs(15))
            // Keep the TLS connection open between hotkey presses: saves a handshake per request.
            .pool_idle_timeout(None)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_while_idle(true)
            .build()
            .expect("http client");
        Self { client }
    }

    /// Opens the connection ahead of the first hotkey press. Result is irrelevant.
    pub async fn warm_up(&self) {
        let _ = self.client.get(URL).send().await;
    }

    pub async fn translate(&self, text: &str, target: &str, fallback: &str) -> Result<Translation, String> {
        let (out, src) = self.request(text, target).await?;
        if src.eq_ignore_ascii_case(target) && !fallback.eq_ignore_ascii_case(target) {
            let (out, src) = self.request(text, fallback).await?;
            return Ok(Translation { text: out, src, tl: fallback.into() });
        }
        Ok(Translation { text: out, src, tl: target.into() })
    }

    /// One retry on network errors and 5xx. 4xx (e.g. 429 rate limit) fails at once.
    async fn request(&self, text: &str, tl: &str) -> Result<(String, String), String> {
        let mut err = String::new();
        for _ in 0..2 {
            let res = self
                .client
                .post(URL)
                .query(&[("client", "gtx"), ("sl", "auto"), ("tl", tl), ("dt", "t"), ("dj", "1")])
                .form(&[("q", text)])
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let body = r.text().await.map_err(|e| format!("Network error: {e}"))?;
                    return parse(&body);
                }
                Ok(r) if r.status().is_server_error() => err = format!("Google error: HTTP {}", r.status()),
                Ok(r) => return Err(format!("Google refused the request: HTTP {}", r.status())),
                Err(e) => err = format!("Network error: {e}"),
            }
        }
        Err(err)
    }
}

fn parse(body: &str) -> Result<(String, String), String> {
    let r: Resp = serde_json::from_str(body).map_err(|e| format!("Unexpected response from Google: {e}"))?;
    Ok((r.sentences.into_iter().map(|s| s.trans).collect(), r.src))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real response captured 2026-11-01, trimmed of debug fields.
    const SAMPLE: &str = r#"{"sentences":[{"trans":"Xin chào thế giới. ","orig":"Hello world. ","backend":3},{"trans":"Bạn hôm nay thế nào?\n","orig":"How are you today?\n","backend":10},{"trans":"Dòng thứ hai.","orig":"Second line.","backend":3}],"src":"en","confidence":1.0,"spell":{},"ld_result":{"srclangs":["en"]}}"#;

    #[test]
    fn parses_sentences_and_source() {
        let (text, src) = parse(SAMPLE).unwrap();
        assert_eq!(text, "Xin chào thế giới. Bạn hôm nay thế nào?\nDòng thứ hai.");
        assert_eq!(src, "en");
    }

    #[test]
    fn bad_body_is_error() {
        assert!(parse("<html>captcha</html>").is_err());
    }

    // Hits the network. Run: cargo test -- --ignored
    #[test]
    #[ignore]
    fn live() {
        let t = Translator::new();
        tauri::async_runtime::block_on(async {
            let r = t.translate("Hello world", "vi", "en").await.unwrap();
            assert_eq!((r.src.as_str(), r.tl.as_str()), ("en", "vi"));
            assert!(!r.text.is_empty());
            // Vietnamese input with target vi falls back to English.
            let r = t.translate("Xin chào các bạn, hôm nay trời đẹp quá", "vi", "en").await.unwrap();
            assert_eq!((r.src.as_str(), r.tl.as_str()), ("vi", "en"));
            println!("{r:?}");
            // Warm connection latency: should be one round trip, no TLS handshake.
            let start = std::time::Instant::now();
            t.translate("Speed check", "vi", "en").await.unwrap();
            println!("warm request: {:?}", start.elapsed());
        });
    }
}

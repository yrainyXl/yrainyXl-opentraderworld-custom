//! China A-share market connector — served by the cn-market service.
//!
//! This connector has no direct access to Tencent/Sina; it talks to the private
//! `cn-market` HTTP service (Python/FastAPI) which wraps Tencent as the primary source
//! and Sina as the fallback. Keyless — no API key required.
//!
//! - `1d` bars: Tencent fqkline (qfq) via cn-market `/bars/{symbol}?timeframe=1d`
//! - `1m` bars: Tencent mkline via cn-market `/bars/{symbol}?timeframe=1m`
//! - search: Tencent smartbox via cn-market `/search?q=`
//!
//! The service base URL comes from `OTW_CN_MARKET_URL` (Docker: `http://cn-market:18080`),
//! defaulting to `http://127.0.0.1:18080` for local development.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use time::OffsetDateTime;

use otw_store::histdata::Bar;

use super::{Capability, Chunk, Connector, SymbolHit};

pub struct China;

static CAP: Capability = Capability {
    provider: "china",
    label: "China Market",
    website: "https://gu.qq.com",
    docs_url: "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get",
    rate_limit: "Keyless (no API key). Primary source: Tencent; automatic fallback to Sina. \
                 Aggressive polling may get an IP throttled; the Watchlist default cadence is fine.",
    required_secrets: &[],
    asset_types: &["equity"],
    timeframes: &["1d", "1m"],
    // Tencent qfq (前复权) daily bars are split/dividend-adjusted.
    adjusted: true,
    // Tencent fqkline serves ~800 daily bars per request.
    max_bars_per_req: 800,
    // Keyless sources; aggressive polling may get an IP throttled, so keep a
    // conservative 1s minimum between requests (matches the rate_limit note).
    min_interval_ms: 1_000,
    searchable: true,
};

/// cn-market service base URL, from `OTW_CN_MARKET_URL` (defaults to localhost:18080).
fn base_url() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| {
        std::env::var("OTW_CN_MARKET_URL").unwrap_or_else(|_| "http://127.0.0.1:18080".to_string())
    })
}

/// Number of bars needed to cover `[from, to)` at `step_secs`, clamped to a sane page.
fn count_for(from: OffsetDateTime, to: OffsetDateTime, step_secs: i64) -> u32 {
    let span = (to - from).whole_seconds().max(0) / step_secs + 2;
    span.clamp(1, CAP.max_bars_per_req as i64) as u32
}

#[async_trait::async_trait]
impl Connector for China {
    fn capability(&self) -> &'static Capability {
        &CAP
    }

    async fn fetch_chunk(
        &self,
        client: &reqwest::Client,
        _secrets: &HashMap<String, String>,
        ticker: &str,
        _asset_type: &str,
        timeframe: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Chunk> {
        let (step_secs, tf) = match timeframe {
            "1d" => (86_400, "1d"),
            "1m" => (60, "1m"),
            other => return Err(anyhow!("{} does not support timeframe {other}", CAP.label)),
        };
        let count = count_for(from, to, step_secs);
        let url = format!(
            "{}/bars/{}?timeframe={}&count={}",
            base_url(),
            ticker.trim(),
            tf,
            count
        );
        let body: Value = crate::rate::send(CAP.provider, client.get(&url))
            .await
            .with_context(|| format!("cn-market bars request for {ticker}"))?
            .error_for_status()
            .with_context(|| format!("cn-market bars status for {ticker}"))?
            .json()
            .await
            .context("cn-market bars decode")?;

        let arr = body
            .get("bars")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("cn-market: no bars (bad symbol?)"))?;

        let mut bars = Vec::with_capacity(arr.len());
        for b in arr {
            let ts = b
                .get("ts")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow!("cn-market bar ts"))?;
            // The connector contract is [from, to): filter out bars outside the window.
            if ts < from.unix_timestamp() || ts >= to.unix_timestamp() {
                continue;
            }
            bars.push(Bar {
                ts: OffsetDateTime::from_unix_timestamp(ts)?,
                open: b.get("open").and_then(Value::as_f64).unwrap_or(0.0),
                high: b.get("high").and_then(Value::as_f64).unwrap_or(0.0),
                low: b.get("low").and_then(Value::as_f64).unwrap_or(0.0),
                close: b.get("close").and_then(Value::as_f64).unwrap_or(0.0),
                volume: b.get("volume").and_then(Value::as_f64).unwrap_or(0.0),
                adj_open: None,
                adj_high: None,
                adj_low: None,
                adj_close: None,
            });
        }
        Ok(Chunk { bars })
    }

    async fn search_symbols(
        &self,
        client: &reqwest::Client,
        _secrets: &HashMap<String, String>,
        query: &str,
        _asset_type: &str,
        limit: usize,
    ) -> Result<Vec<SymbolHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/search?q={}",
            base_url(),
            urlencoding(q)
        );
        let body: Value = crate::rate::send(CAP.provider, client.get(&url))
            .await
            .context("cn-market search request")?
            .error_for_status()
            .context("cn-market search status")?
            .json()
            .await
            .context("cn-market search decode")?;
        let arr = body
            .as_array()
            .ok_or_else(|| anyhow!("cn-market: search did not return a list"))?;
        Ok(arr
            .iter()
            .filter_map(|h| {
                let symbol = h.get("symbol").and_then(Value::as_str)?;
                Some(SymbolHit {
                    symbol: symbol.to_string(),
                    name: h.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    asset_type: h
                        .get("asset_type")
                        .and_then(Value::as_str)
                        .unwrap_or("equity")
                        .to_string(),
                    exchange: h.get("exchange").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            })
            .take(limit)
            .collect())
    }
}

/// Minimal percent-encoding (same policy as the other connectors).
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_covers_the_window() {
        let from = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let to = from + time::Duration::days(70);
        assert_eq!(count_for(from, to, 86_400), 72);
        // A tiny intraday window stays >= 1 and a huge one is clamped to the page size.
        assert_eq!(count_for(from, from + time::Duration::minutes(1), 60), 3);
        assert_eq!(count_for(from, from + time::Duration::days(5000), 86_400), 800);
    }

    #[test]
    fn urlencoding_keeps_cn_chars_encoded() {
        assert_eq!(urlencoding("601118"), "601118");
        assert_eq!(urlencoding("海南"), "%E6%B5%B7%E5%8D%97");
    }
}

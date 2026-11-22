//! TqSdk's DIFF stream (`wss://free-openmd.shinnytech.com/t/md/front/mobile`): one `set_chart`
//! names every market of the workspace, and the client drives the exchange — the server answers
//! each `peek_message` with every pack queued since the previous one. The feed subscribes once per
//! connection and lets `ws::Keepalive::Text` send the peek: each peek returns queued kline packs
//! (never mistaken for a pong), so the forming candles update within one quiet period.

use crate::model::{Kline, Timeframe};
use crate::service::net::ws::{Keepalive, Session};
use serde_json::json;
use std::time::Duration;

const HOST: &str = "free-openmd.shinnytech.com";

/// `None` when TqSdk has no stream for `base` (only intraday and daily bars stream).
pub(super) fn session(symbols: &[String], base: Timeframe) -> Option<Session> {
    let duration_ns = base_duration_ns(base)?;
    let subscribe = vec![
        json!({
            "aid": "set_chart",
            "chart_id": "DGV_live",
            "ins_list": symbols.join(","),
            "duration": duration_ns,
            "view_width": 2,
        })
        .to_string(),
        json!({"aid": "peek_message"}).to_string(),
    ];
    Some(Session {
        url: format!("wss://{HOST}/t/md/front/mobile"),
        host: HOST.to_owned(),
        port: 443,
        subscribe,
        // A peek answers within a second of being sent; the silence cap only has to cover the
        // quiet stretches between the server's pushes.
        max_data_silence: Duration::from_secs(60),
        keepalive: Keepalive::Text {
            ping: r#"{"aid":"peek_message"}"#,
            // TqSdk never answers with a leading `pong`; every answer is a data pack.
            pong: "\u{0}pong-never",
        },
    })
}

/// The native duration of `base` in nanoseconds, as TqSdk's `set_chart` wants it. Only the
/// intervals the charts stream live are here; longer ones fold from the minute bars.
fn base_duration_ns(base: Timeframe) -> Option<i64> {
    let ms = base.nominal_ms();
    if ms <= 0 {
        return None;
    }
    Some(ms * 1_000_000)
}

/// One `rtn_data` pack: the queued DIFFs` kline updates for the chart's markets. A DIFF names
/// each market separately (`klines.<symbol>.<duration>.data.<id>`), so one frame can carry
/// several markets; the newest bar of the requested one wins.
pub(super) fn parse_kline(text: &str, base: Timeframe) -> Option<(String, Kline)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let duration_key = (base.nominal_ms() * 1_000_000).to_string();
    let packs = value.get("data")?.as_array()?;
    let mut best: Option<(String, Kline)> = None;
    for pack in packs {
        let klines = pack.get("klines")?.as_object()?;
        for (symbol, durations) in klines {
            let bars = durations.get(&duration_key)?.get("data")?.as_object()?;
            for (id, bar) in bars {
                let kline = crate::service::market::tqsdk::parse_kline_entry(bar, base.nominal_ms())?;
                if best.as_ref().is_none_or(|(_, old)| {
                    id.parse::<i64>().unwrap_or(i64::MAX) >= 0 && kline.open_time >= old.open_time
                }) {
                    best = Some((symbol.clone(), kline));
                }
            }
        }
    }
    best
}

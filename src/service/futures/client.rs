//! TqSdk futures trade WebSocket client (`req_login`, `insert_order`, `cancel_order`, DIFF state).

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

use super::types::{
    BrokerServerInfo, FuturesAccount, FuturesOrder, FuturesPosition, FuturesSnapshot, FuturesTrade,
    InsertFuturesOrder,
};
use crate::service::market::tqsdk::{
    TqWebSocket, USER_AGENT, authenticate_session, connect_ws_with_headers, merge_diff,
};

const LOOP_READ_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub enum TradeCommand {
    InsertOrder {
        order_id: String,
        request: InsertFuturesOrder,
    },
    CancelOrder {
        order_id: String,
    },
    Disconnect,
}

pub enum TradeSessionEvent {
    LoggedIn,
    Snapshot(Box<FuturesSnapshot>),
    Notice(String),
    Disconnected(Option<String>),
}

pub struct TqTradeHandle {
    cmd_tx: mpsc::Sender<TradeCommand>,
    stop: Arc<AtomicBool>,
}

impl TqTradeHandle {
    pub fn send_order(&self, order_id: String, request: InsertFuturesOrder) -> Result<(), String> {
        self.cmd_tx
            .send(TradeCommand::InsertOrder { order_id, request })
            .map_err(|_| "Futures trade session is not connected".to_owned())
    }

    pub fn cancel_order(&self, order_id: String) -> Result<(), String> {
        self.cmd_tx
            .send(TradeCommand::CancelOrder { order_id })
            .map_err(|_| "Futures trade session is not connected".to_owned())
    }

    pub fn disconnect(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(TradeCommand::Disconnect);
    }
}

impl Drop for TqTradeHandle {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Resolves the futures broker trade WebSocket URL from Shinny's broker directory.
pub fn resolve_td_url(
    access_token: &str,
    shinny_user: &str,
    broker_id: &str,
    account_id: &str,
) -> Result<BrokerServerInfo, String> {
    let url = format!(
        "https://files.shinnytech.com/{broker_id}.json?account_id={}&auth={}",
        urlencoding_component(account_id),
        urlencoding_component(shinny_user),
    );
    let agent: ureq::Agent = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let response = agent
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(|error| format!("Failed to look up broker {broker_id}: {error}"))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(format!(
            "Broker {broker_id} is not supported (HTTP {status})"
        ));
    }
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("Failed to read broker directory: {error}"))?;
    let map: HashMap<String, BrokerServerInfo> = serde_json::from_str(&body)
        .map_err(|error| format!("Invalid broker directory response: {error}"))?;
    map.get(broker_id)
        .cloned()
        .ok_or_else(|| format!("Broker {broker_id} does not support TqSdk login"))
}

fn urlencoding_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn next_order_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    format!("DV{ts:x}{seq:04x}")
}

pub fn authenticate_and_resolve_td(
    shinny_user: &str,
    broker_id: &str,
    account_id: &str,
) -> Result<(String, BrokerServerInfo), String> {
    let (token, _) = authenticate_session().map_err(|e| e.to_string())?;
    let info = resolve_td_url(&token, shinny_user, broker_id, account_id)?;
    Ok((token, info))
}

/// Connects to the futures trade gateway, sends `req_login`, and spawns the DIFF event loop.
pub fn connect_trade_session(
    td_url: String,
    access_token: String,
    broker_id: String,
    account_id: String,
    password: String,
    on_event: impl Fn(TradeSessionEvent) + Send + 'static,
) -> Result<TqTradeHandle, String> {
    let auth = format!("Bearer {access_token}");
    let mut socket = connect_ws_with_headers(
        &td_url,
        &[
            ("User-Agent", USER_AGENT),
            ("Accept", "application/json"),
            ("Authorization", &auth),
            (
                "Sec-WebSocket-Extensions",
                "permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            ),
        ],
    )?;
    socket.set_read_timeout(LOOP_READ_TIMEOUT);

    send_json(
        &mut socket,
        &json!({
            "aid": "req_login",
            "bid": broker_id,
            "user_name": account_id,
            "password": password,
        }),
    )?;
    send_json(&mut socket, &json!({ "aid": "peek_message" }))?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<TradeCommand>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let is_sim = broker_id.contains("模拟") || broker_id.eq_ignore_ascii_case("simnow");

    std::thread::Builder::new()
        .name(format!("tqsdk-trade-{account_id}"))
        .spawn(move || {
            run_trade_loop(
                socket, broker_id, account_id, is_sim, cmd_rx, stop_flag, on_event,
            );
        })
        .map_err(|error| format!("Failed to spawn trade thread: {error}"))?;

    Ok(TqTradeHandle { cmd_tx, stop })
}

fn send_json(socket: &mut TqWebSocket, payload: &Value) -> Result<(), String> {
    socket
        .send_text(&payload.to_string())
        .map_err(|error| format!("Trade WebSocket send failed: {error}"))
}

fn run_trade_loop(
    mut socket: TqWebSocket,
    broker_id: String,
    account_id: String,
    is_sim: bool,
    cmd_rx: mpsc::Receiver<TradeCommand>,
    stop: Arc<AtomicBool>,
    on_event: impl Fn(TradeSessionEvent) + Send + 'static,
) {
    let mut state = json!({});
    let mut logged_in = false;
    let mut disconnect_reason = None;

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                TradeCommand::InsertOrder { order_id, request } => {
                    let (exchange_id, instrument_id) = split_symbol(&request.symbol);
                    let time_condition = match request.price_type {
                        super::types::FuturesPriceType::Any => "IOC",
                        super::types::FuturesPriceType::Limit => "GFD",
                    };
                    let payload = json!({
                        "aid": "insert_order",
                        "user_id": account_id,
                        "order_id": order_id,
                        "exchange_id": exchange_id,
                        "instrument_id": instrument_id,
                        "direction": request.direction.as_tqsdk(),
                        "offset": request.offset.as_tqsdk(),
                        "volume": request.volume,
                        "price_type": request.price_type.as_tqsdk(),
                        "limit_price": request.limit_price,
                        "volume_condition": "ANY",
                        "time_condition": time_condition,
                    });
                    if let Err(error) = send_json(&mut socket, &payload) {
                        on_event(TradeSessionEvent::Notice(error));
                    }
                }
                TradeCommand::CancelOrder { order_id } => {
                    let payload = json!({
                        "aid": "cancel_order",
                        "user_id": account_id,
                        "order_id": order_id,
                    });
                    if let Err(error) = send_json(&mut socket, &payload) {
                        on_event(TradeSessionEvent::Notice(error));
                    }
                }
                TradeCommand::Disconnect => {
                    stop.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }

        match socket.read_text() {
            Ok(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value.get("aid").and_then(Value::as_str) != Some("rtn_data") {
                    continue;
                }
                let mut merged = false;
                if let Some(data) = value.get("data").and_then(Value::as_array) {
                    for patch in data {
                        if let Some(notifies) = patch.get("notify").and_then(Value::as_object) {
                            for item in notifies.values() {
                                if let Some(content) = item.get("content").and_then(Value::as_str) {
                                    on_event(TradeSessionEvent::Notice(content.to_owned()));
                                }
                            }
                        }
                        merge_diff(&mut state, patch.clone());
                        merged = true;
                    }
                }
                if merged {
                    let snapshot =
                        extract_snapshot_from_diff(&state, &broker_id, &account_id, is_sim);
                    if !logged_in && snapshot.trading_day.is_some() {
                        logged_in = true;
                        on_event(TradeSessionEvent::LoggedIn);
                    }
                    on_event(TradeSessionEvent::Snapshot(Box::new(snapshot)));
                    let _ = send_json(&mut socket, &json!({ "aid": "peek_message" }));
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                disconnect_reason = Some(format!("Trade WebSocket error: {error}"));
                break;
            }
        }
    }

    socket.close();
    on_event(TradeSessionEvent::Disconnected(disconnect_reason));
}

pub fn split_symbol(symbol: &str) -> (&str, &str) {
    symbol.split_once('.').unwrap_or(("", symbol))
}

pub fn extract_snapshot_from_diff(
    state: &Value,
    broker_id: &str,
    account_id: &str,
    is_sim: bool,
) -> FuturesSnapshot {
    let trade_node = state.pointer(&format!("/trade/{account_id}"));
    let trading_day = trade_node
        .and_then(|n| n.pointer("/session/trading_day"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let account = trade_node
        .and_then(|n| n.pointer("/accounts/CNY"))
        .and_then(|val| serde_json::from_value::<FuturesAccount>(val.clone()).ok());

    let mut positions = Vec::new();
    if let Some(pos_map) = trade_node
        .and_then(|n| n.get("positions"))
        .and_then(Value::as_object)
    {
        for (sym, val) in pos_map {
            if let Ok(mut pos) = serde_json::from_value::<FuturesPosition>(val.clone())
                && (pos.volume_long > 0 || pos.volume_short > 0)
            {
                pos.symbol = sym.clone();
                positions.push(pos);
            }
        }
        positions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    }

    let mut orders = Vec::new();
    if let Some(ord_map) = trade_node
        .and_then(|n| n.get("orders"))
        .and_then(Value::as_object)
    {
        for val in ord_map.values() {
            if let Ok(mut order) = serde_json::from_value::<FuturesOrder>(val.clone()) {
                order.symbol = if order.exchange_id.is_empty() {
                    order.instrument_id.clone()
                } else {
                    format!("{}.{}", order.exchange_id, order.instrument_id)
                };
                orders.push(order);
            }
        }
        orders.sort_by_key(|a| std::cmp::Reverse(a.insert_date_time));
    }

    let mut trades = Vec::new();
    if let Some(tr_map) = trade_node
        .and_then(|n| n.get("trades"))
        .and_then(Value::as_object)
    {
        for val in tr_map.values() {
            if let Ok(mut trade) = serde_json::from_value::<FuturesTrade>(val.clone()) {
                trade.symbol = if trade.exchange_id.is_empty() {
                    trade.instrument_id.clone()
                } else {
                    format!("{}.{}", trade.exchange_id, trade.instrument_id)
                };
                trades.push(trade);
            }
        }
        trades.sort_by_key(|a| std::cmp::Reverse(a.trade_date_time));
    }

    FuturesSnapshot {
        broker_id: broker_id.to_owned(),
        account_id: account_id.to_owned(),
        is_sim,
        trading_day,
        account,
        positions,
        orders,
        trades,
        last_notice: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_account_positions_orders_and_trades_from_diff_state() {
        let state = json!({
            "trade": {
                "123456": {
                    "session": { "trading_day": "20261116" },
                    "accounts": {
                        "CNY": {
                            "user_id": "123456",
                            "currency": "CNY",
                            "balance": 1_050_000.0,
                            "available": 980_000.0,
                            "margin": 70_000.0,
                            "float_profit": 1200.0,
                            "risk_ratio": 0.0666
                        }
                    },
                    "positions": {
                        "SHFE.rb2610": {
                            "exchange_id": "SHFE",
                            "instrument_id": "rb2610",
                            "volume_long": 2,
                            "volume_long_today": 1,
                            "volume_long_his": 1,
                            "open_price_long": 3100.0,
                            "last_price": 3160.0,
                            "float_profit_long": 1200.0,
                            "margin_long": 7000.0
                        },
                        "DCE.m2609": {
                            "exchange_id": "DCE",
                            "instrument_id": "m2609",
                            "volume_long": 0,
                            "volume_short": 0
                        }
                    },
                    "orders": {
                        "ORD1": {
                            "order_id": "ORD1",
                            "exchange_id": "SHFE",
                            "instrument_id": "rb2610",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume_orign": 2,
                            "volume_left": 1,
                            "price_type": "LIMIT",
                            "limit_price": 3105.0,
                            "status": "ALIVE",
                            "insert_date_time": 1_731_632_400_000_000_000_i64
                        }
                    },
                    "trades": {
                        "TRD1": {
                            "trade_id": "TRD1",
                            "order_id": "ORD1",
                            "exchange_id": "SHFE",
                            "instrument_id": "rb2610",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume": 1,
                            "price": 3100.0,
                            "trade_date_time": 1_731_632_400_000_000_000_i64,
                            "commission": 3.5
                        }
                    }
                }
            }
        });

        let snap = extract_snapshot_from_diff(&state, "快期模拟", "123456", true);
        assert_eq!(snap.trading_day.as_deref(), Some("20261116"));
        assert_eq!(snap.account.as_ref().unwrap().balance, 1_050_000.0);
        assert_eq!(snap.positions.len(), 1);
        assert_eq!(snap.positions[0].symbol, "SHFE.rb2610");
        assert_eq!(snap.orders.len(), 1);
        assert_eq!(snap.orders[0].symbol, "SHFE.rb2610");
        assert_eq!(snap.trades.len(), 1);
        assert_eq!(snap.trades[0].commission, 3.5);
    }
}

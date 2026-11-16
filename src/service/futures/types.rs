//! Domain types for TqSdk (天勤 / Shinny) futures trading state and requests.

use serde::{Deserialize, Deserializer, Serialize};

fn deserialize_f64_or_zero<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Ok(s.trim().parse::<f64>().unwrap_or(0.0)),
        _ => Ok(0.0),
    }
}

fn deserialize_i64_or_zero<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<i64>::deserialize(deserializer)?.unwrap_or(0))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FuturesOrderDirection {
    Buy,
    Sell,
}

impl FuturesOrderDirection {
    pub fn as_tqsdk(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }

    pub fn from_tqsdk(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("SELL") {
            Self::Sell
        } else {
            Self::Buy
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FuturesOffset {
    #[default]
    Open,
    Close,
    CloseToday,
}

impl FuturesOffset {
    pub fn as_tqsdk(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Close => "CLOSE",
            Self::CloseToday => "CLOSETODAY",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FuturesPriceType {
    Limit,
    Any,
}

impl FuturesPriceType {
    pub fn as_tqsdk(self) -> &'static str {
        match self {
            Self::Limit => "LIMIT",
            Self::Any => "ANY",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InsertFuturesOrder {
    pub symbol: String,
    pub direction: FuturesOrderDirection,
    pub offset: FuturesOffset,
    pub volume: i64,
    pub price_type: FuturesPriceType,
    pub limit_price: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BrokerServerInfo {
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub broker_type: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FuturesAccount {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub currency: String,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub available: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub close_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub commission: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub deposit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub float_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub frozen_commission: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub frozen_margin: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub margin: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub position_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub pre_balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub risk_ratio: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub static_balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub withdraw: f64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FuturesPosition {
    #[serde(skip)]
    pub symbol: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_long_today: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_long_his: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_long: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_long_frozen: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_short_today: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_short_his: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_short: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_short_frozen: i64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub open_price_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub open_price_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub position_price_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub position_price_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub last_price: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub float_profit_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub float_profit_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub float_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub margin_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub margin_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub margin: f64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FuturesOrder {
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(skip)]
    pub symbol: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub offset: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_orign: i64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume_left: i64,
    #[serde(default)]
    pub price_type: String,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub limit_price: f64,
    #[serde(default)]
    pub time_condition: String,
    #[serde(default)]
    pub volume_condition: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub insert_date_time: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub last_msg: String,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub frozen_margin: f64,
}

impl FuturesOrder {
    pub fn is_alive(&self) -> bool {
        self.status.eq_ignore_ascii_case("ALIVE")
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FuturesTrade {
    #[serde(default)]
    pub trade_id: String,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(skip)]
    pub symbol: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub offset: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume: i64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub price: f64,
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub trade_date_time: i64,
    #[serde(default, deserialize_with = "deserialize_f64_or_zero")]
    pub commission: f64,
}

#[derive(Clone, Debug, Default)]
pub struct FuturesSnapshot {
    pub broker_id: String,
    pub account_id: String,
    pub is_sim: bool,
    pub trading_day: Option<String>,
    pub account: Option<FuturesAccount>,
    pub positions: Vec<FuturesPosition>,
    pub orders: Vec<FuturesOrder>,
    pub trades: Vec<FuturesTrade>,
    pub last_notice: Option<String>,
}

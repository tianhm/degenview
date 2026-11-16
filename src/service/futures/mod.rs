//! TqSdk futures trading service (broker directory lookup and DIFF trade WebSocket session).

pub mod client;
pub mod types;

pub use client::{
    TqTradeHandle, TradeSessionEvent, authenticate_and_resolve_td, connect_trade_session,
    next_order_id,
};
pub use types::{
    FuturesAccount, FuturesOffset, FuturesOrder, FuturesOrderDirection, FuturesPosition,
    FuturesPriceType, FuturesSnapshot, FuturesTrade, InsertFuturesOrder,
};

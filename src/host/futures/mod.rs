//! Host for TqSdk futures trading: owns the live trade WebSocket handle, the latest DIFF
//! snapshot, and the user's open/close offset preference.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock::lock;
pub use crate::service::futures::{
    FuturesAccount, FuturesOffset, FuturesOrder, FuturesOrderDirection, FuturesPosition,
    FuturesPriceType, FuturesSnapshot, FuturesTrade, InsertFuturesOrder, TqTradeHandle,
    TradeSessionEvent, authenticate_and_resolve_td, connect_trade_session, next_order_id,
};
pub use crate::service::keychain::api_keys::TqSdkTradeCredentials;
use crate::service::keychain::api_keys::{TqSdkTradeCredentialsStore, tqsdk_credentials};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FuturesConnection {
    #[default]
    NotConfigured,
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

struct Inner {
    connection: FuturesConnection,
    snapshot: FuturesSnapshot,
    handle: Option<TqTradeHandle>,
    generation: u64,
    offset: FuturesOffset,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            connection: FuturesConnection::NotConfigured,
            snapshot: FuturesSnapshot::default(),
            handle: None,
            generation: 0,
            offset: FuturesOffset::Open,
        }
    }
}

pub struct FuturesHost {
    inner: Mutex<Inner>,
    revision: Arc<AtomicU64>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl FuturesHost {
    pub fn new(revision: Arc<AtomicU64>) -> Arc<Self> {
        let configured = TqSdkTradeCredentialsStore::shared().is_configured();
        let initial = if configured {
            FuturesConnection::Disconnected
        } else {
            FuturesConnection::NotConfigured
        };
        Arc::new(Self {
            inner: Mutex::new(Inner {
                connection: initial,
                ..Inner::default()
            }),
            revision,
            wake: Mutex::new(None),
        })
    }

    pub fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.wake) = Some(wake);
    }

    pub fn attach(&self, ctx: &eframe::egui::Context) {
        let ctx = ctx.clone();
        self.set_wake(Arc::new(move || ctx.request_repaint()));
    }

    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        if let Some(wake) = lock(&self.wake).as_ref().cloned() {
            wake();
        }
    }

    pub fn connection(&self) -> FuturesConnection {
        lock(&self.inner).connection.clone()
    }

    pub fn snapshot(&self) -> FuturesSnapshot {
        lock(&self.inner).snapshot.clone()
    }

    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    pub fn offset(&self) -> FuturesOffset {
        lock(&self.inner).offset
    }

    pub fn set_offset(&self, offset: FuturesOffset) {
        lock(&self.inner).offset = offset;
        self.bump();
    }

    pub fn connect_saved(self: &Arc<Self>) {
        let trade = TqSdkTradeCredentialsStore::shared().credentials();
        if !trade.is_configured() {
            let mut inner = lock(&self.inner);
            inner.connection = FuturesConnection::NotConfigured;
            drop(inner);
            self.bump();
            return;
        }
        self.connect_with(trade);
    }

    pub fn connect_with(self: &Arc<Self>, trade: TqSdkTradeCredentials) {
        if !trade.is_configured() {
            return;
        }
        let generation = {
            let mut inner = lock(&self.inner);
            inner.handle = None;
            inner.generation = inner.generation.saturating_add(1);
            inner.connection = FuturesConnection::Connecting;
            inner.snapshot = FuturesSnapshot {
                broker_id: trade.broker_id.clone(),
                account_id: trade.account_id.clone(),
                is_sim: trade.broker_id.contains("模拟")
                    || trade.broker_id.eq_ignore_ascii_case("simnow"),
                ..FuturesSnapshot::default()
            };
            inner.generation
        };
        self.bump();

        let host = Arc::clone(self);
        std::thread::Builder::new()
            .name("tqsdk-trade-connect".to_owned())
            .spawn(move || {
                let Some((shinny_user, _)) = tqsdk_credentials() else {
                    let mut inner = lock(&host.inner);
                    if inner.generation == generation {
                        inner.connection = FuturesConnection::Failed(
                            "Set up your Shinny TqSdk account in Settings first".to_owned(),
                        );
                    }
                    drop(inner);
                    host.bump();
                    return;
                };
                let (token, server) = match authenticate_and_resolve_td(
                    &shinny_user,
                    &trade.broker_id,
                    &trade.account_id,
                ) {
                    Ok(pair) => pair,
                    Err(err) => {
                        let mut inner = lock(&host.inner);
                        if inner.generation == generation {
                            inner.connection = FuturesConnection::Failed(err);
                        }
                        drop(inner);
                        host.bump();
                        return;
                    }
                };

                let host_events = Arc::clone(&host);
                let handle = connect_trade_session(
                    server.url,
                    token,
                    trade.broker_id,
                    trade.account_id,
                    trade.password,
                    move |event| host_events.on_event(generation, event),
                );
                match handle {
                    Ok(handle) => {
                        let mut inner = lock(&host.inner);
                        if inner.generation == generation {
                            inner.handle = Some(handle);
                        }
                    }
                    Err(err) => {
                        let mut inner = lock(&host.inner);
                        if inner.generation == generation {
                            inner.connection = FuturesConnection::Failed(err);
                        }
                        drop(inner);
                        host.bump();
                    }
                }
            })
            .ok();
    }

    fn on_event(&self, generation: u64, event: TradeSessionEvent) {
        let mut inner = lock(&self.inner);
        if inner.generation != generation {
            return;
        }
        match event {
            TradeSessionEvent::LoggedIn => {
                inner.connection = FuturesConnection::Connected;
            }
            TradeSessionEvent::Snapshot(mut snap) => {
                snap.last_notice = inner.snapshot.last_notice.clone();
                if snap.trading_day.is_some() {
                    inner.connection = FuturesConnection::Connected;
                }
                inner.snapshot = *snap;
            }
            TradeSessionEvent::Notice(msg) => {
                inner.snapshot.last_notice = Some(msg);
            }
            TradeSessionEvent::Disconnected(reason) => {
                inner.handle = None;
                inner.connection = match reason {
                    Some(err) => FuturesConnection::Failed(err),
                    None => FuturesConnection::Disconnected,
                };
            }
        }
        drop(inner);
        self.bump();
    }

    pub fn disconnect(&self) {
        let mut inner = lock(&self.inner);
        inner.generation = inner.generation.saturating_add(1);
        inner.handle = None;
        inner.connection = if TqSdkTradeCredentialsStore::shared().is_configured() {
            FuturesConnection::Disconnected
        } else {
            FuturesConnection::NotConfigured
        };
        drop(inner);
        self.bump();
    }

    pub fn save_and_connect(self: &Arc<Self>, trade: TqSdkTradeCredentials) -> Result<(), String> {
        TqSdkTradeCredentialsStore::shared()
            .save(&trade)
            .map_err(|e| e.0)?;
        if trade.is_configured() {
            self.connect_with(trade);
        } else {
            self.disconnect();
        }
        Ok(())
    }

    pub fn submit_order(&self, request: InsertFuturesOrder) -> Result<String, String> {
        let inner = lock(&self.inner);
        let Some(handle) = inner.handle.as_ref() else {
            return Err("Futures account is not connected".to_owned());
        };
        let order_id = next_order_id();
        handle.send_order(order_id.clone(), request)?;
        Ok(order_id)
    }

    pub fn cancel_order(&self, order_id: &str) -> Result<(), String> {
        let inner = lock(&self.inner);
        let Some(handle) = inner.handle.as_ref() else {
            return Err("Futures account is not connected".to_owned());
        };
        handle.cancel_order(order_id.to_owned())
    }
}

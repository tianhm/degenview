//! TqSdk futures account, as a [`TradingBackend`].

use std::sync::Arc;

use degenview_brokers::model::ticket::parse_decimal;
use degenview_brokers::{Decimal, MarketRef, Side};

use crate::host::futures::{FuturesConnection, FuturesHost};
use crate::model::DataSource;
use crate::model::broker::chart::Unavailable as ChartUnavailable;
use crate::model::broker::venue::TradingVenue;
use crate::service::futures::{
    FuturesOffset, FuturesOrder, FuturesOrderDirection, FuturesPriceType, InsertFuturesOrder,
};
use crate::service::market::tqsdk::asset_meta;
use crate::trading::backend::TradingBackend;
use crate::trading::draft::{
    CancelResult, DraftCheck, DraftField, DraftIssue, Estimate, PreviewResult, SubmitResult,
    Submitted, TradeDraft,
};
use crate::trading::model::{
    AccountStats, AccountStrip, Amount, BackendState, BalanceRow, Binding, ChartLines, ChartOrder,
    FillRow, Hero, HeroNote, MarketAccess, MarketRules, Note, OrderClass, OrderRow, QuoteView,
    RowStatus, StatCell, StatTone, TicketCaps, TimeInForce, Tone, Trend, Unavailable,
    UnavailableKind, VenueInfo, VenueKind,
};
use crate::trading::task::Task;

pub struct FuturesBackend {
    host: Arc<FuturesHost>,
}

impl FuturesBackend {
    pub fn new(host: Arc<FuturesHost>) -> Self {
        Self { host }
    }
}

fn dec(value: f64) -> Decimal {
    if !value.is_finite() {
        return Decimal::ZERO;
    }
    format!("{value:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .parse::<Decimal>()
        .unwrap_or_default()
}

fn format_cny(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_owned();
    }
    format!("¥{value:.2}")
}

fn resolve_tradable_symbol(symbol: &str) -> Result<String, String> {
    if symbol.starts_with("KQ.m@") || symbol.starts_with("KQ.i@") {
        if let Some(meta) = asset_meta(symbol)
            && let Some(underlying) = meta.underlying_symbol
            && !underlying.is_empty()
        {
            return Ok(underlying);
        }
        return Err(
            "Continuous/index contract cannot be traded directly; open its main contract"
                .to_owned(),
        );
    }
    Ok(symbol.to_owned())
}

fn futures_caps() -> TicketCaps {
    TicketCaps {
        order_classes: vec![OrderClass::Limit, OrderClass::Market],
        time_in_force: vec![TimeInForce::Day, TimeInForce::ImmediateOrCancel],
        post_only: false,
        reduce_only: true,
        brackets: false,
        quote_sized_market: false,
        shorting: true,
        edit_in_place: false,
    }
}

fn order_status(order: &FuturesOrder) -> RowStatus {
    if order.is_alive() {
        if order.volume_left < order.volume_orign {
            RowStatus {
                label: "Partly filled",
                tone: Tone::Good,
            }
        } else {
            RowStatus {
                label: "Working",
                tone: Tone::Good,
            }
        }
    } else if order.volume_left == 0 && order.volume_orign > 0 {
        RowStatus {
            label: "Filled",
            tone: Tone::Good,
        }
    } else if order.last_msg.contains("撤") || order.last_msg.to_lowercase().contains("cancel") {
        RowStatus {
            label: "Cancelled",
            tone: Tone::Neutral,
        }
    } else {
        RowStatus {
            label: "Finished",
            tone: Tone::Neutral,
        }
    }
}

fn offset_label(raw: &str) -> &'static str {
    if raw.eq_ignore_ascii_case("CLOSETODAY") {
        "Close today"
    } else if raw.eq_ignore_ascii_case("CLOSE") {
        "Close"
    } else {
        "Open"
    }
}

impl TradingBackend for FuturesBackend {
    fn info(&self) -> VenueInfo {
        let snap = self.host.snapshot();
        let name = if snap.broker_id.is_empty() {
            "TqSdk Futures".to_owned()
        } else {
            format!("TqSdk · {}", snap.broker_id)
        };
        let kind = if snap.is_sim || snap.broker_id.is_empty() {
            VenueKind::Test
        } else {
            VenueKind::Live
        };
        VenueInfo {
            venue: TradingVenue::TqSdk,
            name,
            kind,
            summary: "Chinese futures via TqSdk / Shinny".to_owned(),
            needs_no_credentials: false,
        }
    }

    fn state(&self) -> BackendState {
        match self.host.connection() {
            FuturesConnection::NotConfigured | FuturesConnection::Disconnected => {
                BackendState::NotConnected
            }
            FuturesConnection::Connecting => BackendState::Connecting,
            FuturesConnection::Connected => BackendState::Ready,
            FuturesConnection::Failed(reason) => BackendState::NeedsAttention(reason),
        }
    }

    fn account(&self) -> Option<AccountStrip> {
        let snap = self.host.snapshot();
        if snap.account_id.is_empty() {
            return None;
        }
        Some(AccountStrip {
            label: if snap.broker_id.is_empty() {
                snap.account_id
            } else {
                format!("{} · {}", snap.broker_id, snap.account_id)
            },
            equity: snap
                .account
                .as_ref()
                .map(|acc| Amount::new(dec(acc.balance), "CNY")),
        })
    }

    fn stats(&self) -> Option<AccountStats> {
        let snap = self.host.snapshot();
        let acc = snap.account?;
        let float_trend = if acc.float_profit > 0.0 {
            Trend::Up
        } else if acc.float_profit < 0.0 {
            Trend::Down
        } else {
            Trend::Flat
        };
        let float_tone = if acc.float_profit > 0.0 {
            StatTone::Profit
        } else if acc.float_profit < 0.0 {
            StatTone::Loss
        } else {
            StatTone::Neutral
        };
        let risk_pct = acc.risk_ratio * 100.0;
        let risk_tone = if risk_pct >= 80.0 {
            StatTone::Critical
        } else if risk_pct >= 50.0 {
            StatTone::Thin
        } else {
            StatTone::Neutral
        };
        Some(AccountStats {
            hero: Hero {
                title: "Equity".to_owned(),
                value: format_cny(acc.balance),
                help: Some("Dynamic futures account equity (CNY)".to_owned()),
                note: Some(HeroNote {
                    trend: float_trend,
                    text: format_cny(acc.float_profit),
                    tone: float_tone,
                    label: "float P&L".to_owned(),
                }),
            },
            cells: vec![
                StatCell {
                    title: "Available".to_owned(),
                    value: format_cny(acc.available),
                    tone: StatTone::Neutral,
                    help: "Free funds available to open new positions".to_owned(),
                },
                StatCell {
                    title: "Margin".to_owned(),
                    value: format_cny(acc.margin),
                    tone: StatTone::Neutral,
                    help: "Margin held by open futures positions".to_owned(),
                },
                StatCell {
                    title: "Risk".to_owned(),
                    value: format!("{risk_pct:.1}%"),
                    tone: risk_tone,
                    help: "Margin / Equity ratio".to_owned(),
                },
            ],
        })
    }

    fn available(&self, _market: &MarketRef, _side: Side) -> Option<Amount> {
        let snap = self.host.snapshot();
        let acc = snap.account?;
        Some(Amount::new(dec(acc.available), "CNY"))
    }

    fn market(&self, market: &MarketRef) -> MarketAccess {
        if market.source != "tqsdk" {
            return MarketAccess::Unavailable(Unavailable {
                kind: UnavailableKind::NotThisVenue,
                text: "TqSdk Futures only trades TqSdk futures contracts".to_owned(),
            });
        }
        let symbol = match resolve_tradable_symbol(&market.symbol) {
            Ok(sym) => sym,
            Err(reason) => {
                return MarketAccess::Unavailable(Unavailable {
                    kind: UnavailableKind::Blocked,
                    text: reason,
                });
            }
        };
        let meta = asset_meta(&symbol);
        let tick = meta
            .as_ref()
            .and_then(|m| (m.price_tick > 0.0).then_some(dec(m.price_tick)))
            .unwrap_or(Decimal::ONE);
        let name = meta
            .as_ref()
            .filter(|m| !m.name.is_empty())
            .map_or_else(|| symbol.clone(), |m| format!("{symbol} · {}", m.name));
        MarketAccess::Ready(Box::new(MarketRules {
            name,
            base_asset: symbol,
            quote_asset: "CNY".to_owned(),
            price_increment: tick,
            size_increment: Decimal::ONE,
            min_size: Decimal::ONE,
            min_notional: Decimal::ZERO,
            caps: futures_caps(),
        }))
    }

    fn quote(&self, market: &MarketRef) -> Option<QuoteView> {
        let symbol = resolve_tradable_symbol(&market.symbol).ok()?;
        let snap = self.host.snapshot();
        let pos = snap.positions.iter().find(|p| p.symbol == symbol)?;
        (pos.last_price > 0.0).then(|| QuoteView {
            bid: None,
            ask: None,
            last: Some(dec(pos.last_price)),
        })
    }

    fn orders(&self) -> Vec<OrderRow> {
        let snap = self.host.snapshot();
        snap.orders
            .iter()
            .map(|order| {
                let side = match FuturesOrderDirection::from_tqsdk(&order.direction) {
                    FuturesOrderDirection::Buy => Side::Buy,
                    FuturesOrderDirection::Sell => Side::Sell,
                };
                let kind = if order.price_type.eq_ignore_ascii_case("ANY") {
                    format!("Market · {}", offset_label(&order.offset))
                } else {
                    format!("Limit · {}", offset_label(&order.offset))
                };
                let filled = (order.volume_orign - order.volume_left).max(0);
                let working = order.is_alive();
                OrderRow {
                    id: order.order_id.clone(),
                    market: order.symbol.clone(),
                    side,
                    kind,
                    price: (order.limit_price > 0.0).then(|| dec(order.limit_price)),
                    trigger: None,
                    filled: Decimal::from(filled),
                    size: Some(Decimal::from(order.volume_orign)),
                    spend: None,
                    status: order_status(order),
                    created_ms: (order.insert_date_time > 0)
                        .then_some(order.insert_date_time / 1_000_000),
                    working,
                    can_cancel: working,
                    can_edit: false,
                    note: (!order.last_msg.is_empty()).then(|| order.last_msg.clone()),
                    edit_hint: Some(
                        "Futures orders cannot be edited in place; cancel and re-enter".to_owned(),
                    ),
                    source: Some(DataSource::TqSdk),
                    tag: None,
                    unconfirmed: false,
                    pending: false,
                    fees: Decimal::ZERO,
                }
            })
            .collect()
    }

    fn check(&self, market: &MarketRef, draft: &TradeDraft) -> DraftCheck {
        let mut issues = Vec::new();
        let missing = draft.missing();
        let size = parse_decimal(&draft.size_text).ok().flatten();
        if let Some(size) = size {
            if size <= Decimal::ZERO || !size.fract().is_zero() {
                issues.push(DraftIssue::new(
                    DraftField::Size,
                    "Futures size must be a positive whole number of lots",
                ));
            }
        } else if !draft.size_text.trim().is_empty() {
            issues.push(DraftIssue::new(DraftField::Size, "Invalid lot count"));
        }

        let price = if draft.uses_limit_price() {
            match parse_decimal(&draft.price_text) {
                Ok(Some(p)) if p > Decimal::ZERO => Some(p),
                Ok(Some(_)) => {
                    issues.push(DraftIssue::new(
                        DraftField::Price,
                        "Price must be greater than zero",
                    ));
                    None
                }
                Ok(None) => None,
                Err(_) => {
                    issues.push(DraftIssue::new(DraftField::Price, "Invalid price"));
                    None
                }
            }
        } else {
            None
        };

        let meta = resolve_tradable_symbol(&market.symbol)
            .ok()
            .and_then(|sym| asset_meta(&sym));
        let mult = meta
            .as_ref()
            .filter(|m| m.volume_multiple > 0.0)
            .map_or(1.0, |m| m.volume_multiple);
        let notional = size.zip(price).map(|(s, p)| s * p * dec(mult));

        DraftCheck {
            issues,
            missing,
            estimate: Estimate {
                reference: price,
                notional,
                fee: None,
                margin: None,
            },
        }
    }

    fn max_size(
        &self,
        _market: &MarketRef,
        _side: Side,
        _price: Option<Decimal>,
    ) -> Option<Decimal> {
        None
    }

    fn binding(&self) -> Binding {
        let snap = self.host.snapshot();
        Binding {
            account: snap.account_id,
            generation: self.host.generation(),
        }
    }

    fn preview(
        &self,
        market: &MarketRef,
        draft: &TradeDraft,
        binding: &Binding,
    ) -> Task<Result<PreviewResult, String>> {
        if binding.generation != self.host.generation() {
            return Task::ready(Err("Connection changed; check the order again".to_owned()));
        }
        let check = self.check(market, draft);
        if !check.is_complete() {
            let problems = check.issues.into_iter().map(|i| i.text).collect();
            return Task::ready(Ok(PreviewResult {
                accepted: false,
                problems,
                ..PreviewResult::default()
            }));
        }
        Task::ready(Ok(PreviewResult {
            accepted: true,
            total: check.estimate.notional,
            fee: check.estimate.fee,
            average_price: check.estimate.reference,
            problems: Vec::new(),
            warnings: Vec::new(),
        }))
    }

    fn submit(
        &self,
        market: &MarketRef,
        draft: &TradeDraft,
        binding: &Binding,
    ) -> Task<SubmitResult> {
        if binding.generation != self.host.generation() {
            return Task::ready(Err(
                "Connection changed since preview; order not sent".to_owned()
            ));
        }
        let symbol = match resolve_tradable_symbol(&market.symbol) {
            Ok(sym) => sym,
            Err(err) => return Task::ready(Err(err)),
        };
        let Ok(Some(size_dec)) = parse_decimal(&draft.size_text) else {
            return Task::ready(Err("Enter a valid whole number of lots".to_owned()));
        };
        let Ok(volume) = i64::try_from(size_dec.trunc()) else {
            return Task::ready(Err("Lot count out of range".to_owned()));
        };
        if volume <= 0 {
            return Task::ready(Err("Lot count must be at least 1".to_owned()));
        }

        let (price_type, limit_price) = match draft.class {
            OrderClass::Market => (FuturesPriceType::Any, 0.0),
            OrderClass::Limit => {
                let Ok(Some(price_dec)) = parse_decimal(&draft.price_text) else {
                    return Task::ready(Err("Enter a valid limit price".to_owned()));
                };
                let price_f64 = price_dec.to_string().parse::<f64>().unwrap_or(0.0);
                if price_f64 <= 0.0 {
                    return Task::ready(Err("Limit price must be greater than zero".to_owned()));
                }
                (FuturesPriceType::Limit, price_f64)
            }
            _ => {
                return Task::ready(Err(
                    "Only Limit and Market orders are supported for futures".to_owned(),
                ));
            }
        };

        let direction = match draft.side {
            Side::Buy => FuturesOrderDirection::Buy,
            Side::Sell => FuturesOrderDirection::Sell,
        };
        let offset = if draft.reduce_only {
            match self.host.offset() {
                FuturesOffset::CloseToday => FuturesOffset::CloseToday,
                _ => FuturesOffset::Close,
            }
        } else {
            self.host.offset()
        };

        let result = self
            .host
            .submit_order(InsertFuturesOrder {
                symbol,
                direction,
                offset,
                volume,
                price_type,
                limit_price,
            })
            .map(|id| Submitted { id });
        Task::ready(result)
    }

    fn notes(&self) -> Vec<Note> {
        let snap = self.host.snapshot();
        let mut out = Vec::new();
        if let FuturesConnection::Failed(err) = self.host.connection() {
            out.push(Note {
                text: err,
                error: true,
            });
        }
        if let Some(notice) = snap.last_notice {
            out.push(Note {
                text: notice,
                error: false,
            });
        }
        out
    }

    fn recent_fills(&self) -> Vec<FillRow> {
        let snap = self.host.snapshot();
        snap.trades
            .iter()
            .map(|t| FillRow {
                id: t.trade_id.clone(),
                time_ms: t.trade_date_time / 1_000_000,
                market: t.symbol.clone(),
                side: match FuturesOrderDirection::from_tqsdk(&t.direction) {
                    FuturesOrderDirection::Buy => Side::Buy,
                    FuturesOrderDirection::Sell => Side::Sell,
                },
                price: format!("{:.2}", t.price),
                size: Decimal::from(t.volume),
                fee: dec(t.commission),
                order: t.order_id.clone(),
            })
            .collect()
    }

    fn balances(&self) -> Vec<BalanceRow> {
        let snap = self.host.snapshot();
        let Some(acc) = snap.account else {
            return Vec::new();
        };
        vec![BalanceRow {
            asset: if acc.currency.is_empty() {
                "CNY".to_owned()
            } else {
                acc.currency
            },
            available: dec(acc.available),
            held: dec(acc.margin + acc.frozen_margin),
        }]
    }

    fn chart_unavailable(
        &self,
        market: Option<&MarketRef>,
        replay: bool,
    ) -> Option<ChartUnavailable> {
        if replay {
            return Some(ChartUnavailable::InReplay);
        }
        let Some(market) = market else {
            return Some(ChartUnavailable::NotAMarket);
        };
        match self.market(market) {
            MarketAccess::Ready(_) => None,
            MarketAccess::Unavailable(u) => Some(match u.kind {
                UnavailableKind::NotThisVenue => ChartUnavailable::OtherSource,
                UnavailableKind::Waiting => ChartUnavailable::NotConnected,
                UnavailableKind::Blocked => {
                    ChartUnavailable::MarketNotTradable("Futures contract is not tradable")
                }
            }),
        }
    }

    fn chart_lines(&self, market: &MarketRef) -> Option<ChartLines> {
        if market.source != "tqsdk" {
            return None;
        }
        let symbol = resolve_tradable_symbol(&market.symbol).ok()?;
        let snap = self.host.snapshot();
        let orders = snap
            .orders
            .iter()
            .filter(|o| o.is_alive() && o.symbol == symbol && o.limit_price > 0.0)
            .map(|o| {
                let side = match FuturesOrderDirection::from_tqsdk(&o.direction) {
                    FuturesOrderDirection::Buy => Side::Buy,
                    FuturesOrderDirection::Sell => Side::Sell,
                };
                ChartOrder {
                    id: o.order_id.clone(),
                    side,
                    tag: if o.price_type.eq_ignore_ascii_case("ANY") {
                        "MKT"
                    } else {
                        "LMT"
                    },
                    price: dec(o.limit_price),
                    price_text: format!("{:.2}", o.limit_price),
                    remaining_text: format!("{} lots", o.volume_left),
                    note: None,
                    can_edit: false,
                    can_cancel: true,
                }
            })
            .collect();
        let tick =
            asset_meta(&symbol).and_then(|m| (m.price_tick > 0.0).then_some(dec(m.price_tick)));
        Some(ChartLines {
            price_increment: tick,
            orders,
        })
    }

    fn cancel(&self, order_id: &str) -> Task<CancelResult> {
        Task::ready(self.host.cancel_order(order_id))
    }
}

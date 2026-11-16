//! Bottom trading panel for TqSdk Chinese futures: positions, orders, fills, and account summary.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::host::futures::{
    FuturesAccount, FuturesConnection, FuturesHost, FuturesOffset, FuturesOrder,
    FuturesOrderDirection, FuturesPosition, FuturesPriceType, FuturesSnapshot, FuturesTrade,
    InsertFuturesOrder,
};
use crate::trading::model::AccountStats;
use crate::ui::broker::badge;
use crate::ui::icons::Icon;
use crate::ui::kit;
use crate::ui::theme;
use crate::ui::trade::panel_header::{self, Chip, Header, HeaderEvent, PanelEvent, VenueBar};
use crate::ui::trade::stats_strip;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FuturesTab {
    #[default]
    Positions,
    Orders,
    Fills,
    Account,
}

#[derive(Default)]
pub struct FuturesPanelState {
    tab: FuturesTab,
    notice: Option<(String, bool, Instant)>,
}

pub fn ui(
    ui: &mut egui::Ui,
    state: &mut FuturesPanelState,
    host: &FuturesHost,
    bar: &VenueBar,
    stats: Option<&AccountStats>,
) -> Vec<PanelEvent> {
    let mut actions = Vec::new();
    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
    badge::rule(ui);

    let connection = host.connection();
    let snap = host.snapshot();
    let active_positions = snap
        .positions
        .iter()
        .filter(|p| p.volume_long > 0 || p.volume_short > 0)
        .count();
    let working_orders = snap.orders.iter().filter(|o| o.is_alive()).count();

    let chip = Chip {
        label: if snap.account_id.is_empty() {
            "TqSdk Futures".to_owned()
        } else if snap.broker_id.is_empty() {
            snap.account_id.clone()
        } else {
            format!("{} · {}", snap.broker_id, snap.account_id)
        },
        sub: snap.is_sim.then(|| "SIM".to_owned()),
        tip: "TqSdk futures account".to_owned(),
        rows: Vec::new(),
        add: None,
    };
    let tabs = [
        kit::TabItem {
            value: FuturesTab::Positions,
            title: "Positions",
            icon: Icon::Layers,
            count: active_positions,
        },
        kit::TabItem {
            value: FuturesTab::Orders,
            title: "Orders",
            icon: Icon::List,
            count: working_orders,
        },
        kit::TabItem {
            value: FuturesTab::Fills,
            title: "Fills",
            icon: Icon::Check,
            count: snap.trades.len(),
        },
        kit::TabItem {
            value: FuturesTab::Account,
            title: "Account",
            icon: Icon::ChartLine,
            count: 0,
        },
    ];
    let header_events = panel_header::show(
        ui,
        &Header {
            bar,
            chip: &chip,
            tabs: &tabs,
            overlay: None,
            can_order: matches!(connection, FuturesConnection::Connected),
            more: &[],
            settings_tip: "TqSdk futures trade account settings",
        },
        &mut state.tab,
    );
    for event in header_events {
        if let HeaderEvent::Panel(ev) = event {
            actions.push(ev);
        }
    }
    kit::panel_divider(ui);

    match &connection {
        FuturesConnection::NotConfigured => {
            if empty_with_settings(
                ui,
                "Configure your TqSdk futures trade account in Settings ▸ TqSdk.",
            ) {
                actions.push(PanelEvent::OpenSettings);
            }
            return actions;
        }
        FuturesConnection::Disconnected => {
            if empty_with_settings(
                ui,
                "TqSdk futures account is disconnected. Connect in Settings ▸ TqSdk.",
            ) {
                actions.push(PanelEvent::OpenSettings);
            }
            return actions;
        }
        FuturesConnection::Connecting if snap.account.is_none() => {
            ui.centered_and_justified(|ui| {
                ui.horizontal(|ui| {
                    ui.add(kit::loading::LoadingIndicator::new().size(14.0));
                    ui.add_space(8.0);
                    ui.label(kit::text(
                        "Connecting to futures broker…",
                        kit::size::BODY,
                        kit::REGULAR,
                        theme::text_secondary(),
                    ));
                });
            });
            return actions;
        }
        FuturesConnection::Failed(reason) if snap.account.is_none() => {
            if empty_with_settings(ui, &format!("Connection failed: {reason}")) {
                actions.push(PanelEvent::OpenSettings);
            }
            return actions;
        }
        _ => {}
    }

    if let Some((text, is_error, at)) = &state.notice {
        if at.elapsed() < Duration::from_secs(10) {
            notice_bar(ui, text, *is_error);
        } else {
            state.notice = None;
        }
    } else if let Some(notice) = &snap.last_notice {
        notice_bar(ui, notice, false);
    }

    stats_strip::show(ui, stats);
    kit::panel_divider(ui);
    offset_toolbar(ui, host);
    kit::panel_divider(ui);

    match state.tab {
        FuturesTab::Positions => positions_tab(ui, state, host, &snap),
        FuturesTab::Orders => orders_tab(ui, state, host, &snap),
        FuturesTab::Fills => fills_tab(ui, &snap),
        FuturesTab::Account => account_tab(ui, &snap),
    }

    actions
}

fn empty_with_settings(ui: &mut egui::Ui, message: &str) -> bool {
    let mut open_settings = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(kit::text(
                message,
                kit::size::BODY,
                kit::REGULAR,
                theme::text_secondary(),
            ));
            ui.add_space(10.0);
            if kit::sheet_button(ui, "Open Settings", false, true).clicked() {
                open_settings = true;
            }
        });
    });
    open_settings
}

fn notice_bar(ui: &mut egui::Ui, text: &str, is_error: bool) {
    let color = if is_error {
        kit::RED
    } else {
        theme::text_secondary()
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.label(kit::text(text, kit::size::CAPTION, kit::REGULAR, color));
        });
}

fn offset_toolbar(ui: &mut egui::Ui, host: &FuturesHost) {
    let current = host.offset();
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(kit::text(
                    "Order Offset",
                    kit::size::CAPTION,
                    kit::MEDIUM,
                    theme::text_secondary(),
                ));
                ui.add_space(10.0);
                for (offset, label) in [
                    (FuturesOffset::Open, "Open (开仓)"),
                    (FuturesOffset::Close, "Close (平仓)"),
                    (FuturesOffset::CloseToday, "Close Today (平今)"),
                ] {
                    let selected = current == offset;
                    if ui
                        .selectable_label(
                            selected,
                            kit::text(
                                label,
                                kit::size::CAPTION,
                                if selected {
                                    kit::SEMIBOLD
                                } else {
                                    kit::REGULAR
                                },
                                if selected {
                                    theme::text_primary()
                                } else {
                                    theme::text_secondary()
                                },
                            ),
                        )
                        .clicked()
                    {
                        host.set_offset(offset);
                    }
                    ui.add_space(6.0);
                }
            });
        });
}

fn positions_tab(
    ui: &mut egui::Ui,
    state: &mut FuturesPanelState,
    host: &FuturesHost,
    snap: &FuturesSnapshot,
) {
    let active: Vec<&FuturesPosition> = snap
        .positions
        .iter()
        .filter(|p| p.volume_long > 0 || p.volume_short > 0)
        .collect();
    if active.is_empty() {
        empty_message(ui, "No open futures positions.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("futures_positions")
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    egui::Grid::new("futures_positions_grid")
                        .num_columns(7)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for h in [
                                "Contract",
                                "Side",
                                "Lots (Today/Yest)",
                                "Avg Price",
                                "Float P&L",
                                "Margin",
                                "Action",
                            ] {
                                ui.label(kit::text(
                                    h,
                                    kit::size::CAPTION,
                                    kit::SEMIBOLD,
                                    theme::text_secondary(),
                                ));
                            }
                            ui.end_row();

                            for pos in active {
                                if pos.volume_long > 0 {
                                    position_row(
                                        ui,
                                        state,
                                        host,
                                        pos,
                                        true,
                                        pos.volume_long,
                                        pos.volume_long_today,
                                        pos.volume_long_his,
                                        pos.open_price_long,
                                        pos.float_profit_long,
                                        pos.margin_long,
                                    );
                                }
                                if pos.volume_short > 0 {
                                    position_row(
                                        ui,
                                        state,
                                        host,
                                        pos,
                                        false,
                                        pos.volume_short,
                                        pos.volume_short_today,
                                        pos.volume_short_his,
                                        pos.open_price_short,
                                        pos.float_profit_short,
                                        pos.margin_short,
                                    );
                                }
                            }
                        });
                });
        });
}

#[allow(clippy::too_many_arguments)]
fn position_row(
    ui: &mut egui::Ui,
    state: &mut FuturesPanelState,
    host: &FuturesHost,
    pos: &FuturesPosition,
    is_long: bool,
    lots: i64,
    today: i64,
    his: i64,
    avg_price: f64,
    float_profit: f64,
    margin: f64,
) {
    let symbol = if pos.symbol.is_empty() {
        format!("{}.{}", pos.exchange_id, pos.instrument_id)
    } else {
        pos.symbol.clone()
    };
    ui.label(kit::text(
        &symbol,
        kit::size::BODY,
        kit::MEDIUM,
        theme::text_primary(),
    ));
    let (side_text, side_color) = if is_long {
        ("Long", theme::UP)
    } else {
        ("Short", theme::DOWN)
    };
    ui.label(kit::text(
        side_text,
        kit::size::BODY,
        kit::MEDIUM,
        side_color,
    ));
    ui.label(kit::text(
        format!("{lots} ({today}/{his})"),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    ui.label(kit::text(
        format!("{avg_price:.2}"),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    let pnl_color = if float_profit > 0.0 {
        theme::UP
    } else if float_profit < 0.0 {
        theme::DOWN
    } else {
        theme::text_secondary()
    };
    ui.label(kit::text(
        format!("{float_profit:+.2}"),
        kit::size::BODY,
        kit::MEDIUM,
        pnl_color,
    ));
    ui.label(kit::text(
        format!("{margin:.2}"),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_secondary(),
    ));
    if kit::push_button(ui, None, "Close", true).clicked() {
        let close_dir = if is_long {
            FuturesOrderDirection::Sell
        } else {
            FuturesOrderDirection::Buy
        };
        let offset = if today > 0 && his == 0 {
            FuturesOffset::CloseToday
        } else {
            FuturesOffset::Close
        };
        let close_vol = if offset == FuturesOffset::CloseToday {
            today
        } else {
            lots
        };
        let req = InsertFuturesOrder {
            symbol,
            direction: close_dir,
            offset,
            volume: close_vol.max(1),
            price_type: FuturesPriceType::Any,
            limit_price: 0.0,
        };
        match host.submit_order(req) {
            Ok(_) => {
                state.notice = Some(("Close order sent".to_owned(), false, Instant::now()));
            }
            Err(err) => {
                state.notice = Some((err, true, Instant::now()));
            }
        }
    }
    ui.end_row();
}

fn orders_tab(
    ui: &mut egui::Ui,
    state: &mut FuturesPanelState,
    host: &FuturesHost,
    snap: &FuturesSnapshot,
) {
    if snap.orders.is_empty() {
        empty_message(ui, "No futures orders in this session.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("futures_orders")
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    egui::Grid::new("futures_orders_grid")
                        .num_columns(7)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for h in [
                                "Contract", "Side", "Offset", "Price", "Filled", "Status", "Action",
                            ] {
                                ui.label(kit::text(
                                    h,
                                    kit::size::CAPTION,
                                    kit::SEMIBOLD,
                                    theme::text_secondary(),
                                ));
                            }
                            ui.end_row();

                            for order in &snap.orders {
                                order_row(ui, state, host, order);
                            }
                        });
                });
        });
}

fn order_row(
    ui: &mut egui::Ui,
    state: &mut FuturesPanelState,
    host: &FuturesHost,
    order: &FuturesOrder,
) {
    let symbol = if order.exchange_id.is_empty() {
        order.instrument_id.clone()
    } else {
        format!("{}.{}", order.exchange_id, order.instrument_id)
    };
    ui.label(kit::text(
        &symbol,
        kit::size::BODY,
        kit::MEDIUM,
        theme::text_primary(),
    ));
    let side_color = if order.direction == "BUY" {
        theme::UP
    } else {
        theme::DOWN
    };
    ui.label(kit::text(
        &order.direction,
        kit::size::BODY,
        kit::MEDIUM,
        side_color,
    ));
    ui.label(kit::text(
        &order.offset,
        kit::size::BODY,
        kit::REGULAR,
        theme::text_secondary(),
    ));
    let price = if order.price_type == "ANY" || order.limit_price <= 0.0 {
        "Market".to_owned()
    } else {
        format!("{:.2}", order.limit_price)
    };
    ui.label(kit::text(
        price,
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    let filled = order.volume_orign - order.volume_left;
    ui.label(kit::text(
        format!("{filled}/{}", order.volume_orign),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    let status = if order.last_msg.is_empty() {
        order.status.as_str()
    } else {
        order.last_msg.as_str()
    };
    ui.label(kit::text(
        status,
        kit::size::CAPTION,
        kit::REGULAR,
        theme::text_secondary(),
    ));
    if order.is_alive() {
        if kit::push_button(ui, None, "Cancel", true).clicked()
            && let Err(err) = host.cancel_order(&order.order_id)
        {
            state.notice = Some((err, true, Instant::now()));
        }
    } else {
        ui.label("");
    }
    ui.end_row();
}

fn fills_tab(ui: &mut egui::Ui, snap: &FuturesSnapshot) {
    if snap.trades.is_empty() {
        empty_message(ui, "No futures fills in this session.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("futures_fills")
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    egui::Grid::new("futures_fills_grid")
                        .num_columns(6)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for h in ["Contract", "Side", "Offset", "Price", "Lots", "Commission"] {
                                ui.label(kit::text(
                                    h,
                                    kit::size::CAPTION,
                                    kit::SEMIBOLD,
                                    theme::text_secondary(),
                                ));
                            }
                            ui.end_row();

                            for trade in &snap.trades {
                                fill_row(ui, trade);
                            }
                        });
                });
        });
}

fn fill_row(ui: &mut egui::Ui, trade: &FuturesTrade) {
    let symbol = if trade.exchange_id.is_empty() {
        trade.instrument_id.clone()
    } else {
        format!("{}.{}", trade.exchange_id, trade.instrument_id)
    };
    ui.label(kit::text(
        &symbol,
        kit::size::BODY,
        kit::MEDIUM,
        theme::text_primary(),
    ));
    let side_color = if trade.direction == "BUY" {
        theme::UP
    } else {
        theme::DOWN
    };
    ui.label(kit::text(
        &trade.direction,
        kit::size::BODY,
        kit::MEDIUM,
        side_color,
    ));
    ui.label(kit::text(
        &trade.offset,
        kit::size::BODY,
        kit::REGULAR,
        theme::text_secondary(),
    ));
    ui.label(kit::text(
        format!("{:.2}", trade.price),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    ui.label(kit::text(
        format!("{}", trade.volume),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_primary(),
    ));
    ui.label(kit::text(
        format!("{:.2}", trade.commission),
        kit::size::BODY,
        kit::REGULAR,
        theme::text_secondary(),
    ));
    ui.end_row();
}

fn account_tab(ui: &mut egui::Ui, snap: &FuturesSnapshot) {
    let Some(acc) = &snap.account else {
        empty_message(ui, "Waiting for account snapshot…");
        return;
    };
    account_grid(ui, acc, snap.trading_day.as_deref());
}

fn account_grid(ui: &mut egui::Ui, acc: &FuturesAccount, trading_day: Option<&str>) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            egui::Grid::new("futures_account_grid")
                .num_columns(4)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    let rows = [
                        ("Balance (动态权益)", format!("{:.2} CNY", acc.balance)),
                        ("Available (可用资金)", format!("{:.2} CNY", acc.available)),
                        ("Margin (占用保证金)", format!("{:.2} CNY", acc.margin)),
                        (
                            "Frozen Margin (冻结保证金)",
                            format!("{:.2} CNY", acc.frozen_margin),
                        ),
                        (
                            "Float Profit (浮动盈亏)",
                            format!("{:+.2} CNY", acc.float_profit),
                        ),
                        (
                            "Close Profit (平仓盈亏)",
                            format!("{:+.2} CNY", acc.close_profit),
                        ),
                        ("Commission (手续费)", format!("{:.2} CNY", acc.commission)),
                        (
                            "Risk Ratio (风险度)",
                            format!("{:.2}%", acc.risk_ratio * 100.0),
                        ),
                        (
                            "Pre-Balance (昨日权益)",
                            format!("{:.2} CNY", acc.pre_balance),
                        ),
                        ("Trading Day", trading_day.unwrap_or("—").to_owned()),
                    ];
                    for (idx, (label, val)) in rows.iter().enumerate() {
                        ui.label(kit::text(
                            *label,
                            kit::size::CAPTION,
                            kit::REGULAR,
                            theme::text_secondary(),
                        ));
                        ui.label(kit::text(
                            val,
                            kit::size::BODY,
                            kit::MEDIUM,
                            theme::text_primary(),
                        ));
                        if idx % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
        });
}

fn empty_message(ui: &mut egui::Ui, msg: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(kit::text(
            msg,
            kit::size::BODY,
            kit::REGULAR,
            theme::text_secondary(),
        ));
    });
}

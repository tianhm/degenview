//! The live Binance and TqSdk feeds: keeping them on the workspace's charts and folding their candles in.

use crate::model::market::live::{self, LiveMark};
use crate::model::{DataSource, Kline, Timeframe};
use crate::service::market::live::{BinanceLiveFeed, TqSdkLiveFeed};
use crate::workspace::ChartWorkspace;
use eframe::egui;
use std::time::Instant;

impl ChartWorkspace {
    pub(in crate::workspace) fn update_live_feed(&mut self, ctx: &egui::Context) {
        self.update_binance_live_feed(ctx);
        self.update_tqsdk_live_feed(ctx);
    }

    fn update_binance_live_feed(&mut self, ctx: &egui::Context) {
        let mut symbols: Vec<String> = self
            .charts
            .iter()
            .filter(|chart| chart.is_market() && chart.source == DataSource::Binance)
            .map(|chart| chart.symbol.to_uppercase())
            .collect();
        // The markets compared on a chart stream too, once however many overlays name them.
        for symbol in self.overlay_hub.binance_symbols() {
            if !symbols.contains(&symbol) {
                symbols.push(symbol);
            }
        }
        let interval = self.timeframe.interval().to_owned();
        // Binance streams no quarterly or yearly klines; those fold monthly candles.
        let monthly_fold = matches!(self.timeframe, Timeframe::Quarter | Timeframe::Year);
        let signature = (symbols, interval);

        let should_connect = !monthly_fold && !signature.0.is_empty();
        let needs_change = match &self.live_feed {
            Some(feed) => feed.signature() != &signature || !should_connect,
            None => should_connect,
        };
        if needs_change {
            self.live_feed = None;
            if should_connect {
                self.live_feed = Some(BinanceLiveFeed::connect(
                    signature.0.clone(),
                    signature.1.clone(),
                    ctx.clone(),
                ));
            }
        }

        let updates = self
            .live_feed
            .as_ref()
            .map(|feed| feed.inbox.drain())
            .unwrap_or_default();
        let now = Instant::now();
        for (symbol, kline) in updates {
            self.overlay_hub.apply_live(&symbol, &kline, now);
            self.apply_live_kline(DataSource::Binance, &symbol, kline, now);
        }
    }

    fn update_tqsdk_live_feed(&mut self, ctx: &egui::Context) {
        let mut symbols: Vec<String> = Vec::new();
        for chart in &self.charts {
            if chart.is_market()
                && chart.source == DataSource::TqSdk
                && !symbols.contains(&chart.symbol)
            {
                symbols.push(chart.symbol.clone());
            }
        }
        let interval = self.timeframe.interval().to_owned();
        let calendar_fold = matches!(
            self.timeframe,
            Timeframe::Week | Timeframe::Month | Timeframe::Quarter | Timeframe::Year
        );
        let signature = (symbols, interval);

        let should_connect = !calendar_fold && !signature.0.is_empty();
        let needs_change = match &self.tqsdk_live_feed {
            Some(feed) => feed.signature() != &signature || !should_connect,
            None => should_connect,
        };
        if needs_change {
            self.tqsdk_live_feed = None;
            if should_connect {
                self.tqsdk_live_feed = Some(TqSdkLiveFeed::connect(
                    signature.0.clone(),
                    signature.1.clone(),
                    ctx.clone(),
                ));
            }
        }

        let updates = self
            .tqsdk_live_feed
            .as_ref()
            .map(|feed| feed.inbox.drain())
            .unwrap_or_default();
        let now = Instant::now();
        for (symbol, kline) in updates {
            self.apply_live_kline(DataSource::TqSdk, &symbol, kline, now);
        }
    }

    fn apply_live_kline(&mut self, source: DataSource, symbol: &str, kline: Kline, now: Instant) {
        for chart in self.charts.iter_mut().filter(|chart| {
            !(chart.candles.is_empty() && chart.loading)
                && chart.source == source
                && if source == DataSource::TqSdk {
                    chart.symbol == symbol
                } else {
                    chart.symbol.eq_ignore_ascii_case(symbol)
                }
        }) {
            if live::apply_live(&mut chart.candles, kline.clone()) {
                chart.live = Some(LiveMark::new(kline.open_time, now));
                chart.last_data_ms = Some(crate::model::watchlist::now_ms());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::chart::Chart;
    use crate::workspace::test_support::{candle, empty_workspace};

    #[test]
    fn every_chart_of_the_symbol_takes_the_live_bar() {
        let mut workspace = empty_workspace(1);
        for id in 1..=2 {
            let mut chart = Chart::blank(id, "BTCUSDT", DataSource::Binance);
            chart.candles = vec![candle(1, 1.0)];
            workspace.charts.push(chart);
        }
        let mut other = Chart::blank(3, "ETHUSDT", DataSource::Binance);
        other.candles = vec![candle(1, 1.0)];
        workspace.charts.push(other);

        workspace.apply_live_kline(
            DataSource::Binance,
            "btcusdt",
            candle(2, 5.0),
            Instant::now(),
        );

        let lens: Vec<usize> = workspace.charts.iter().map(|c| c.candles.len()).collect();
        assert_eq!(lens, [2, 2, 1]);
        assert!(workspace.charts[0].live.is_some() && workspace.charts[1].live.is_some());
    }

    #[test]
    fn a_late_tick_of_an_older_bar_does_not_unsort_the_chart() {
        let mut workspace = empty_workspace(1);
        let mut chart = Chart::blank(1, "BTCUSDT", DataSource::Binance);
        chart.candles = vec![candle(1, 1.0), candle(2, 2.0)];
        workspace.charts.push(chart);

        workspace.apply_live_kline(
            DataSource::Binance,
            "BTCUSDT",
            candle(1, 9.0),
            Instant::now(),
        );

        assert_eq!(
            workspace.charts[0].candles,
            [candle(1, 1.0), candle(2, 2.0)]
        );
        assert!(workspace.charts[0].live.is_none());
    }
}

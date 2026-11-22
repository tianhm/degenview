//! The Pine indicators that ship inside the app (`indicators/*.pine`). Unlike the demo scripts
//! they are never copied into the user's library: they are always available, offered by the
//! add-indicator picker and the chart's "+" menu, and applied to a chart as a Pine instance whose
//! script id is [`script_id`]. Every one is original work from the published definition of the
//! study, under the app's own license.

use crate::model::indicators::catalog::Family;

/// One bundled indicator: its catalog text and its Pine source.
#[derive(Debug, PartialEq)]
pub struct BundledIndicator {
    /// Stable, used for favorites and the instance's script id. Never reuse or rename one.
    pub id: &'static str,
    pub name: &'static str,
    pub family: Family,
    pub summary: &'static str,
    pub description: &'static str,
    /// Other names a search matches.
    pub aliases: &'static [&'static str],
    /// On the price pane (`true`) or in a pane of its own.
    pub overlay: bool,
    pub author: &'static str,
    pub license: &'static str,
    pub source: &'static str,
}

/// The prefix of a bundled indicator's script id on a chart.
pub const SCRIPT_ID_PREFIX: &str = "builtin:";

/// The script id a chart instance of `indicator` carries.
pub fn script_id(indicator: &BundledIndicator) -> String {
    format!("{SCRIPT_ID_PREFIX}{}", indicator.id)
}

pub fn find(id: &str) -> Option<&'static BundledIndicator> {
    ALL.iter().find(|indicator| indicator.id == id)
}

macro_rules! indicator {
    (
        $id:literal, $name:literal, $family:ident, $overlay:literal,
        $summary:literal, $description:literal, [$($alias:literal),* $(,)?]
    ) => {
        BundledIndicator {
            id: $id,
            name: $name,
            family: Family::$family,
            summary: $summary,
            description: $description,
            aliases: &[$($alias),*],
            overlay: $overlay,
            author: "DegenView",
            license: "GPL-3.0-only",
            source: include_str!(concat!($id, ".pine")),
        }
    };
}

macro_rules! quant_edge_indicator {
    (
        $id:literal, $name:literal, $family:ident, $overlay:literal,
        $summary:literal, $description:literal, [$($alias:literal),* $(,)?]
    ) => {
        BundledIndicator {
            id: $id,
            name: $name,
            family: Family::$family,
            summary: $summary,
            description: $description,
            aliases: &[$($alias),*],
            overlay: $overlay,
            author: "Ahmed (Quant-Edge)",
            license: "MPL-2.0",
            source: include_str!(concat!($id, ".pine")),
        }
    };
}

/// Every bundled indicator: overlays first, then the ones with a pane of their own.
pub static ALL: [BundledIndicator; 36] = [
    indicator!(
        "ichimoku-cloud",
        "Ichimoku Cloud",
        Trend,
        true,
        "Midline averages and a projected cloud that frames the trend.",
        "Ichimoku Kinko Hyo draws a conversion and a base line and a cloud between two leading spans shifted forward. Price above the cloud is bullish, below is bearish, inside is indecision. The lagging span is not drawn.",
        ["ichimoku kinko hyo", "kumo", "tenkan", "kijun"]
    ),
    indicator!(
        "parabolic-sar",
        "Parabolic SAR",
        Trend,
        true,
        "A trailing stop that accelerates toward price.",
        "Wilder's Parabolic Stop and Reverse puts a dot below the bars while the trend is up and above them while it is down, speeding up as the trend runs. A dot that flips side is a reversal signal.",
        ["psar", "stop and reverse", "sar"]
    ),
    indicator!(
        "donchian-channels",
        "Donchian Channels",
        Volatility,
        true,
        "The highest high and lowest low of the last N bars.",
        "Draws the highest high and lowest low over a window with their midpoint. A close beyond a band is a breakout, which is the entry rule of the classic turtle system.",
        ["donchian", "turtle", "price channel"]
    ),
    indicator!(
        "keltner-channels",
        "Keltner Channels",
        Volatility,
        true,
        "An EMA with bands a multiple of the ATR away.",
        "A moving average with bands set by the Average True Range, so the channel follows volatility. Closes outside the channel suggest a strong move; a flat, narrow channel suggests a range.",
        ["keltner", "kc", "atr bands"]
    ),
    indicator!(
        "vwap",
        "VWAP",
        VolumeFlow,
        true,
        "Volume weighted average price, restarted each day, with bands.",
        "The average price weighted by volume since the start of the day, with bands a multiple of the volume-weighted standard deviation away. Traders use it as the fair price of the session. It needs volume.",
        ["volume weighted average price", "anchored vwap"]
    ),
    indicator!(
        "moving-average-ribbon",
        "Moving Average Ribbon",
        Trend,
        true,
        "Six moving averages of increasing length.",
        "Six averages (SMA, EMA, WMA or HMA) from a first length in equal steps. A fanned-out ribbon means a strong trend, a tangle means a range, and the order of the lines flips when the trend does.",
        ["ma ribbon", "ema ribbon", "guppy"]
    ),
    indicator!(
        "williams-fractals",
        "Williams Fractals",
        Structure,
        true,
        "Marks swing highs and lows confirmed by the bars on each side.",
        "A fractal is a bar whose high (or low) is the highest (lowest) of the bars either side of it. Each is confirmed after those bars close, and the mark is placed back on the bar. Fractals are often used as support, resistance and stop levels.",
        ["fractal", "swing high", "swing low", "pivot"]
    ),
    indicator!(
        "linear-regression-bands",
        "Linear Regression Bands",
        Trend,
        true,
        "The least-squares line of recent closes with deviation bands.",
        "Fits a straight line through the last N closes and draws bands a multiple of the standard deviation above and below it. The slope is the trend and the bands show how stretched price is from it.",
        ["linreg", "regression channel", "least squares"]
    ),
    indicator!(
        "macd",
        "MACD",
        Momentum,
        false,
        "Moving Average Convergence Divergence with signal and histogram.",
        "The difference between a fast and a slow EMA, a signal line of that difference, and a histogram of the gap. The histogram is darker while momentum is growing. A MACD line crossing its signal is the classic trigger.",
        ["moving average convergence divergence"]
    ),
    indicator!(
        "stochastic",
        "Stochastic",
        Momentum,
        false,
        "Where the close sits inside the recent range, 0 to 100.",
        "Compares the close with the high-low range of the last N bars, smoothed into %K and %D. Above the upper level is overbought and below the lower level is oversold.",
        ["stoch", "stochastic oscillator", "%k", "%d"]
    ),
    indicator!(
        "stochastic-rsi",
        "Stochastic RSI",
        Momentum,
        false,
        "The Stochastic formula applied to RSI.",
        "Applies the Stochastic to the RSI instead of price. It reaches the ends of its range far more often than RSI itself, so it times turns earlier and gives more false ones.",
        ["stoch rsi", "stochrsi"]
    ),
    indicator!(
        "cci",
        "CCI",
        Momentum,
        false,
        "Commodity Channel Index: distance from the average price.",
        "Measures how far the typical price is from its moving average in units of the mean deviation. Readings beyond +100 or -100 mark an unusually strong move.",
        ["commodity channel index"]
    ),
    indicator!(
        "williams-r",
        "Williams %R",
        Momentum,
        false,
        "Distance of the close below the recent high, 0 to -100.",
        "Shows where the close is relative to the highest high of the last N bars. Above -20 is overbought and below -80 is oversold. It is the Stochastic turned upside down.",
        ["williams percent r", "wpr", "%r"]
    ),
    indicator!(
        "adx-dmi",
        "ADX / DMI",
        Trend,
        false,
        "Trend strength (ADX) with the +DI and -DI direction lines.",
        "ADX measures how strong a trend is, not which way: above about 25 there is a trend. +DI over -DI means the trend is up, the reverse that it is down.",
        [
            "adx",
            "dmi",
            "directional movement index",
            "average directional index"
        ]
    ),
    indicator!(
        "aroon",
        "Aroon",
        Trend,
        false,
        "How recently the highest high and lowest low happened.",
        "Aroon Up and Aroon Down, each 0 to 100, say how many bars ago the highest high and the lowest low were. A strong Aroon Up with a weak Aroon Down is an uptrend.",
        ["aroon up", "aroon down", "aroon oscillator"]
    ),
    indicator!(
        "atr",
        "ATR",
        Volatility,
        false,
        "Average True Range: the typical bar size including gaps.",
        "Wilder's Average True Range, in price units or as a percent of the close. It measures volatility, not direction, and is widely used to size stops and positions.",
        ["average true range", "true range"]
    ),
    indicator!(
        "obv",
        "OBV",
        VolumeFlow,
        false,
        "On-Balance Volume: a running total of signed volume.",
        "Adds the bar's volume on an up close and subtracts it on a down close, with a moving average of the total as its signal. Rising OBV while price is flat suggests accumulation.",
        ["on balance volume"]
    ),
    indicator!(
        "money-flow-index",
        "Money Flow Index",
        VolumeFlow,
        false,
        "A volume-weighted RSI, 0 to 100.",
        "Like RSI but built from the typical price times volume. Above 80 is overbought and below 20 oversold; a new price extreme that MFI does not confirm is a divergence. It needs volume.",
        ["mfi", "volume rsi"]
    ),
    indicator!(
        "chaikin-money-flow",
        "Chaikin Money Flow",
        VolumeFlow,
        false,
        "Buying and selling pressure over N bars, -1 to +1.",
        "Weights each bar's volume by where it closed inside its range and sums over N bars. Above zero means closes lean toward the highs (buying pressure). It needs volume.",
        ["cmf", "chaikin"]
    ),
    indicator!(
        "awesome-oscillator",
        "Awesome Oscillator",
        Momentum,
        false,
        "Fast minus slow average of the bar midpoint.",
        "Bill Williams' histogram: a 5-period average of the midpoint minus a 34-period one. Green bars are rising and red bars falling; crossing zero is a momentum shift.",
        ["ao", "bill williams"]
    ),
    quant_edge_indicator!(
        "qe-custom-indicator",
        "Quant-Edge Candle Counter",
        Trend,
        true,
        "Candle counter, moving average, RSI, and trend mode dashboard.",
        "Analyzes a configurable lookback of candles with moving average, RSI, and candle/range trend mode classification.",
        ["quant-edge", "candle counter", "trend mode"]
    ),
    quant_edge_indicator!(
        "qe-fibonacci-fvg",
        "Quant-Edge Fibonacci + FVG",
        Structure,
        true,
        "Automatic Fibonacci retracement levels combined with Fair Value Gaps.",
        "Detects swing highs/lows to plot Fibonacci retracements alongside bullish and bearish Fair Value Gaps (FVG).",
        ["quant-edge", "fibonacci", "fvg"]
    ),
    quant_edge_indicator!(
        "qe-fibonacci-autosignals",
        "Quant-Edge Fibonacci AutoSignals",
        Structure,
        true,
        "Fibonacci retracement and extension zones with automated buy/sell signals.",
        "Calculates dynamic Fibonacci retracement and extension levels and emits buy/sell signals on level interactions.",
        ["quant-edge", "fibonacci", "autosignals"]
    ),
    quant_edge_indicator!(
        "qe-double-rsi-strategy",
        "Quant-Edge Double RSI Strategy",
        Momentum,
        true,
        "Dual-timeframe RSI strategy combining higher-TF and chart-TF RSI.",
        "Combines a higher-timeframe RSI filter with chart-timeframe RSI entries and exits.",
        ["quant-edge", "double rsi", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-larry-connors-rsi-strategy",
        "Quant-Edge Larry Connors RSI",
        Momentum,
        true,
        "2-period RSI mean-reversion strategy with trend filter.",
        "Classic Larry Connors 2-period RSI pullback strategy entering oversold dips in an uptrend and exiting on strength.",
        ["quant-edge", "connors", "rsi2", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-rsi-divergences",
        "Quant-Edge RSI Divergences",
        Momentum,
        false,
        "RSI oscillator with regular and hidden bullish/bearish divergence detection.",
        "Plots RSI with overbought/oversold bands and marks pivot divergences between price and RSI.",
        ["quant-edge", "rsi divergence"]
    ),
    quant_edge_indicator!(
        "qe-rsi-multilayer-entry",
        "Quant-Edge RSI Multilayer Entry",
        Momentum,
        true,
        "Multi-layer RSI scaling entry indicator with position tracking.",
        "Tracks layered oversold/overbought RSI entries and plots position state on the price chart.",
        ["quant-edge", "rsi multilayer"]
    ),
    quant_edge_indicator!(
        "qe-apex-trend",
        "Quant-Edge Apex Trend",
        Trend,
        true,
        "Apex Trend & Liquidity Master overlay with trend bands and liquidity levels.",
        "Combines trend-following bands with liquidity sweep and structure signals.",
        ["quant-edge", "apex trend", "liquidity"]
    ),
    quant_edge_indicator!(
        "qe-fvg",
        "Quant-Edge Inversion FVG",
        Structure,
        true,
        "Inversion Fair Value Gaps (IFVG) with ICT signals.",
        "Detects bullish and bearish Fair Value Gaps and tracks inverted FVG zones as support/resistance.",
        ["quant-edge", "ifvg", "fvg", "ict"]
    ),
    quant_edge_indicator!(
        "qe-mss-choch-bos",
        "Quant-Edge MSS + CHoCH + BOS",
        Structure,
        true,
        "Market Structure Shift (MSS), Change of Character (CHoCH), and Break of Structure (BOS).",
        "Identifies fractal swing highs/lows, CHoCH/BOS structural breaks, and fair value gaps.",
        ["quant-edge", "mss", "choch", "bos", "market structure"]
    ),
    quant_edge_indicator!(
        "qe-trio-liquidity",
        "Quant-Edge Trio Liquidity",
        VolumeFlow,
        true,
        "Liquidity sweep and close-back confirmation boxes and signals.",
        "Detects swing high/low liquidity sweeps followed by close-back confirmation.",
        ["quant-edge", "liquidity", "sweep"]
    ),
    quant_edge_indicator!(
        "qe-btcusd-5m-strategy",
        "Quant-Edge BTCUSD 5M Strategy",
        Trend,
        true,
        "EMA 200 trend filter with MACD momentum entries for 5-minute charts.",
        "Uses a 200-period EMA trend bias paired with MACD crossovers and risk-managed exits.",
        ["quant-edge", "btcusd", "ema200", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-donchian-pullback",
        "Quant-Edge Donchian Pullback",
        Volatility,
        true,
        "Donchian Channel breakout with pullback entry strategy.",
        "Identifies Donchian Channel trend direction and enters on controlled pullbacks.",
        ["quant-edge", "donchian", "pullback", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-eurusd-1m-strategy",
        "Quant-Edge EURUSD 1M Strategy",
        Momentum,
        true,
        "EMA 9/21 crossover and RSI scalping strategy.",
        "Fast 9/21 EMA scalping strategy filtered by RSI momentum.",
        ["quant-edge", "eurusd", "scalping", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-ivan-strategy",
        "Quant-Edge Ivan Strategy",
        Trend,
        true,
        "Ivan Scherman 200/5 SMA pattern strategy with TP/SL.",
        "Trades the Ivan Scherman moving-average pullback pattern with fixed take-profit and stop-loss.",
        ["quant-edge", "ivan", "sma", "strategy"]
    ),
    quant_edge_indicator!(
        "qe-turtle-strategy",
        "Quant-Edge Turtle Strategy",
        Volatility,
        true,
        "Donchian Turtle strategy with dynamic TP, fixed SL, and 1% risk sizing.",
        "Implements Donchian Channel breakout entries with ATR/fixed stop-loss and dynamic take-profit.",
        ["quant-edge", "turtle", "donchian", "strategy"]
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kline;
    use crate::pine::compiler;
    use crate::pine::compiler::program::SymbolInfo;
    use crate::pine::syntax::token::{DiagnosticSeverity, Limits};
    use crate::pine_studio::library::registry::LibraryRegistry;
    use std::collections::HashSet;

    fn bars(count: usize) -> Vec<Kline> {
        (0..count)
            .map(|i| {
                let t = i as f64;
                let base = 100.0 + (t * 0.17).sin() * 9.0 + (t * 0.05).cos() * 6.0 + t * 0.04;
                Kline {
                    open_time: 1_700_000_000_000 + i as i64 * 3_600_000,
                    open: base - 0.4,
                    high: base + 1.8 + (t * 0.3).sin().abs(),
                    low: base - 1.8 - (t * 0.2).cos().abs(),
                    close: base + 0.5,
                    volume: 900.0 + (t * 0.4).sin().abs() * 700.0,
                    close_time: 1_700_000_000_000 + (i as i64 + 1) * 3_600_000 - 1,
                }
            })
            .collect()
    }

    fn symbol() -> SymbolInfo {
        SymbolInfo {
            ticker: "BTCUSDT".into(),
            ticker_id: "BINANCE:BTCUSDT".into(),
            currency: "USD".into(),
            kind: "crypto".into(),
            base_currency: None,
            ..SymbolInfo::default()
        }
    }

    #[test]
    fn ids_are_unique_and_script_ids_name_them() {
        let ids: HashSet<&str> = ALL.iter().map(|indicator| indicator.id).collect();
        assert_eq!(ids.len(), ALL.len());
        for indicator in &ALL {
            let script_id = script_id(indicator);
            assert_eq!(
                script_id
                    .strip_prefix(SCRIPT_ID_PREFIX)
                    .and_then(find)
                    .map(|i| i.id),
                Some(indicator.id)
            );
        }
    }

    #[test]
    fn every_indicator_compiles_with_no_diagnostics_at_all() {
        let mut problems = Vec::new();
        for indicator in &ALL {
            let program = compiler::compile(indicator.source, Limits::default(), &LibraryRegistry);
            for diagnostic in &program.diagnostics {
                problems.push(format!(
                    "{}: {:?} {}",
                    indicator.name, diagnostic.severity, diagnostic.message
                ));
            }
            let declared_overlay = indicator.source.contains("overlay = true");
            if declared_overlay != indicator.overlay {
                problems.push(format!(
                    "{}: overlay flag disagrees with the script",
                    indicator.name
                ));
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn bounded_oscillators_stay_inside_their_range() {
        // (indicator id, plot title, lowest, highest)
        let ranges: [(&str, &str, f64, f64); 12] = [
            ("stochastic", "%K", 0.0, 100.0),
            ("stochastic", "%D", 0.0, 100.0),
            ("stochastic-rsi", "%K", 0.0, 100.0),
            ("williams-r", "%R", -100.0, 0.0),
            ("money-flow-index", "MFI", 0.0, 100.0),
            ("aroon", "Aroon Up", 0.0, 100.0),
            ("aroon", "Aroon Down", 0.0, 100.0),
            ("chaikin-money-flow", "CMF", -1.0, 1.0),
            ("adx-dmi", "ADX", 0.0, 100.0),
            ("adx-dmi", "+DI", 0.0, 100.0),
            ("adx-dmi", "-DI", 0.0, 100.0),
            ("cci", "CCI", -400.0, 400.0),
        ];
        for (id, title, low, high) in ranges {
            let indicator = find(id).unwrap();
            let (output, _) = crate::host::pine::run_script_checked(
                indicator.source,
                &bars(400),
                &symbol(),
                &std::collections::HashMap::new(),
            );
            let plot = output
                .plots
                .iter()
                .find(|plot| plot.title.as_deref() == Some(title))
                .unwrap_or_else(|| panic!("{id} has no plot {title}"));
            let values: Vec<f64> = plot.values.iter().flatten().copied().collect();
            assert!(
                values.len() > 300,
                "{id} {title}: only {} values",
                values.len()
            );
            for value in &values {
                assert!(
                    (low..=high).contains(value),
                    "{id} {title}: {value} is outside {low}..{high}"
                );
            }
            // A flat line would pass a range check and still be wrong.
            let spread = values.iter().cloned().fold(f64::MIN, f64::max)
                - values.iter().cloned().fold(f64::MAX, f64::min);
            assert!(spread > 1e-6, "{id} {title} is flat");
        }
    }

    #[test]
    fn every_indicator_runs_to_the_end_and_draws_something() {
        let mut problems = Vec::new();
        for indicator in &ALL {
            let (output, failure) = crate::host::pine::run_script_checked(
                indicator.source,
                &bars(300),
                &symbol(),
                &std::collections::HashMap::new(),
            );
            let drawn = output.plots.len() + output.markers.len() + output.hlines.len();
            if drawn == 0 {
                problems.push(format!("{}: draws nothing", indicator.name));
            }
            if let Some(failure) = failure {
                let severity = if failure.severity == DiagnosticSeverity::Error {
                    "error"
                } else {
                    "diagnostic"
                };
                problems.push(format!(
                    "{}: {severity}: {}",
                    indicator.name, failure.message
                ));
            }
            // A plot that is all gaps would be a blank pane.
            for plot in &output.plots {
                if !plot.values.iter().any(|value| value.is_some())
                    && !indicator.name.contains("Fractals")
                {
                    problems.push(format!(
                        "{}: plot {:?} has no values",
                        indicator.name, plot.title
                    ));
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

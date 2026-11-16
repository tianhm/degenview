use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Timeframe {
    Min1,
    Min3,
    Min5,
    Min15,
    Min30,
    Hour,
    Hour4,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Timeframe {
    pub const ALL: [Self; 12] = [
        Self::Min1,
        Self::Min3,
        Self::Min5,
        Self::Min15,
        Self::Min30,
        Self::Hour,
        Self::Hour4,
        Self::Day,
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::Year,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Min1 => "1m",
            Self::Min3 => "3m",
            Self::Min5 => "5m",
            Self::Min15 => "15m",
            Self::Min30 => "30m",
            Self::Hour => "1H",
            Self::Hour4 => "4H",
            Self::Day => "1D",
            Self::Week => "1W",
            Self::Month => "1M",
            Self::Quarter => "3M",
            Self::Year => "1Y",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Min1 => "One-minute candles",
            Self::Min3 => "Three-minute candles",
            Self::Min5 => "Five-minute candles",
            Self::Min15 => "Fifteen-minute candles",
            Self::Min30 => "Thirty-minute candles",
            Self::Hour => "One-hour candles",
            Self::Hour4 => "Four-hour candles",
            Self::Day => "Daily candles",
            Self::Week => "Weekly candles",
            Self::Month => "Monthly candles",
            Self::Quarter => "Quarterly candles",
            Self::Year => "Yearly candles",
        }
    }

    /// The interval token every provider is asked for. Quarters and years are real calendar
    /// buckets: a source with no such bar folds finer ones (`service::fold_calendar`).
    pub const fn interval(self) -> &'static str {
        match self {
            Self::Min1 => "1m",
            Self::Min3 => "3m",
            Self::Min5 => "5m",
            Self::Min15 => "15m",
            Self::Min30 => "30m",
            Self::Hour => "1h",
            Self::Hour4 => "4h",
            Self::Day => "1d",
            Self::Week => "1w",
            Self::Month => "1M",
            Self::Quarter => "3M",
            Self::Year => "1Y",
        }
    }

    /// A bar's nominal length in milliseconds. Calendar timeframes are approximate (a month is 30
    /// days, a quarter 91, a year 365): it is only used where a bar's span is capped by the next
    /// bar's open anyway, such as aligning one market's candles to another's.
    pub const fn nominal_ms(self) -> i64 {
        const MIN: i64 = 60_000;
        const DAY: i64 = 86_400_000;
        match self {
            Self::Min1 => MIN,
            Self::Min3 => 3 * MIN,
            Self::Min5 => 5 * MIN,
            Self::Min15 => 15 * MIN,
            Self::Min30 => 30 * MIN,
            Self::Hour => 60 * MIN,
            Self::Hour4 => 240 * MIN,
            Self::Day => DAY,
            Self::Week => 7 * DAY,
            Self::Month => 30 * DAY,
            Self::Quarter => 91 * DAY,
            Self::Year => 365 * DAY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_timeframe_has_its_own_interval_token() {
        let tokens: Vec<_> = Timeframe::ALL.iter().map(|tf| tf.interval()).collect();
        assert_eq!(
            tokens,
            [
                "1m", "3m", "5m", "15m", "30m", "1h", "4h", "1d", "1w", "1M", "3M", "1Y"
            ]
        );
    }
}

//! Fixed-duration candle intervals and typed stream subscriptions.
use crate::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interval {
    #[serde(rename = "1s")]
    Second,
    #[serde(rename = "1m")]
    Minute,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "15m")]
    FifteenMinutes,
    #[serde(rename = "1h")]
    Hour,
    #[serde(rename = "4h")]
    FourHours,
    #[serde(rename = "1d")]
    Day,
    #[serde(rename = "1w")]
    Week,
}
impl Interval {
    pub fn milliseconds(self) -> i64 {
        match self {
            Self::Second => 1_000,
            Self::Minute => 60_000,
            Self::FiveMinutes => 300_000,
            Self::FifteenMinutes => 900_000,
            Self::Hour => 3_600_000,
            Self::FourHours => 14_400_000,
            Self::Day => 86_400_000,
            Self::Week => 604_800_000,
        }
    }
    /// UTC boundary; weeks start Monday (Unix epoch was a Thursday).
    pub fn floor(self, time: Timestamp) -> Result<Timestamp> {
        let offset = if self == Self::Week { 345_600_000 } else { 0 };
        let shifted = i128::from(time) - offset;
        i64::try_from(
            shifted.div_euclid(i128::from(self.milliseconds())) * i128::from(self.milliseconds())
                + offset,
        )
        .map_err(|_| Error::Invalid("interval boundary overflow".into()))
    }
}
impl FromStr for Interval {
    type Err = Error;
    fn from_str(input: &str) -> Result<Self> {
        match input {
            "1s" => Ok(Self::Second),
            "1m" => Ok(Self::Minute),
            "5m" => Ok(Self::FiveMinutes),
            "15m" => Ok(Self::FifteenMinutes),
            "1h" => Ok(Self::Hour),
            "4h" => Ok(Self::FourHours),
            "1d" => Ok(Self::Day),
            "1w" => Ok(Self::Week),
            _ => Err(Error::Invalid(format!(
                "unsupported fixed interval: {input}"
            ))),
        }
    }
}
impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Second => "1s",
            Self::Minute => "1m",
            Self::FiveMinutes => "5m",
            Self::FifteenMinutes => "15m",
            Self::Hour => "1h",
            Self::FourHours => "4h",
            Self::Day => "1d",
            Self::Week => "1w",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Quotes,
    Trades,
    Book,
    Candles(Interval),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub symbol: String,
    pub channel: Channel,
}

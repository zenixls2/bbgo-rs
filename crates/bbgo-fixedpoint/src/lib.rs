//! Finite, checked eight-decimal arithmetic derived from BBGO's `pkg/fixedpoint`.
//! No float conversion is used for parsing, formatting, or trading arithmetic.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

pub const SCALE: i64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid decimal syntax")]
    Syntax,
    #[error("more than eight non-zero fractional digits")]
    Precision,
    #[error("fixed-point overflow")]
    Overflow,
    #[error("division by zero")]
    DivisionByZero,
    #[error("step must be positive")]
    InvalidStep,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Value(i64);

impl Value {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(SCALE);

    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> i64 {
        self.0
    }
    pub fn from_integer(n: i64) -> Result<Self, Error> {
        n.checked_mul(SCALE).map(Self).ok_or(Error::Overflow)
    }
    pub fn checked_add(self, rhs: Self) -> Result<Self, Error> {
        self.0.checked_add(rhs.0).map(Self).ok_or(Error::Overflow)
    }
    pub fn checked_sub(self, rhs: Self) -> Result<Self, Error> {
        self.0.checked_sub(rhs.0).map(Self).ok_or(Error::Overflow)
    }
    /// Multiplication and division truncate toward zero at eight decimals.
    pub fn checked_mul(self, rhs: Self) -> Result<Self, Error> {
        Self::narrow(i128::from(self.0) * i128::from(rhs.0) / i128::from(SCALE))
    }
    pub fn checked_div(self, rhs: Self) -> Result<Self, Error> {
        if rhs.0 == 0 {
            return Err(Error::DivisionByZero);
        }
        Self::narrow(i128::from(self.0) * i128::from(SCALE) / i128::from(rhs.0))
    }
    fn narrow(raw: i128) -> Result<Self, Error> {
        i64::try_from(raw).map(Self).map_err(|_| Error::Overflow)
    }
    /// Mathematical floor to a positive exchange tick/quantity step.
    pub fn floor_to(self, step: Self) -> Result<Self, Error> {
        if step.0 <= 0 {
            return Err(Error::InvalidStep);
        }
        Self::narrow(i128::from(self.0).div_euclid(i128::from(step.0)) * i128::from(step.0))
    }
    /// Lossy conversion for statistical models only, never order sizing.
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }
}

impl FromStr for Value {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let (negative, s) = if let Some(s) = s.strip_prefix('-') {
            (true, s)
        } else {
            (false, s.strip_prefix('+').unwrap_or(s))
        };
        let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::Syntax);
        }
        let fraction = fraction.trim_end_matches('0');
        if fraction.len() > 8 {
            return Err(Error::Precision);
        }
        let whole: i128 = whole.parse().map_err(|_| Error::Overflow)?;
        let frac: i128 = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<i128>().map_err(|_| Error::Syntax)?
                * 10_i128.pow(8 - fraction.len() as u32)
        };
        let raw = whole
            .checked_mul(i128::from(SCALE))
            .and_then(|v| v.checked_add(frac))
            .ok_or(Error::Overflow)?;
        Self::narrow(if negative { -raw } else { raw })
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw = i128::from(self.0).abs();
        if self.0 < 0 {
            write!(f, "-")?;
        }
        write!(f, "{}", raw / i128::from(SCALE))?;
        let fraction = raw % i128::from(SCALE);
        if fraction != 0 {
            write!(f, ".{}", format!("{fraction:08}").trim_end_matches('0'))?;
        }
        Ok(())
    }
}
impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Decimal strings are mandatory: JSON floating-point numbers lose precision.
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_roundtrip() {
        for raw in [i64::MIN, i64::MAX, 0, 1, -1, SCALE, -SCALE] {
            let v = Value::from_raw(raw);
            assert_eq!(v.to_string().parse::<Value>(), Ok(v));
            assert_eq!(
                serde_json::from_str::<Value>(&serde_json::to_string(&v).unwrap()).unwrap(),
                v
            );
        }
        assert_eq!("0.100000000".parse::<Value>().unwrap().to_string(), "0.1");
        for bad in ["", ".", "NaN", "inf", "1e-8", " 1", "1.2.3"] {
            assert!(bad.parse::<Value>().is_err(), "{bad}");
        }
        assert_eq!("0.000000001".parse::<Value>(), Err(Error::Precision));
    }
    #[test]
    fn checked_arithmetic() {
        let a: Value = "0.1".parse().unwrap();
        assert_eq!(a.checked_mul(a).unwrap().to_string(), "0.01");
        assert_eq!(a.checked_div(Value::ZERO), Err(Error::DivisionByZero));
        assert_eq!(
            Value::from_raw(i64::MAX).checked_add(Value::ONE),
            Err(Error::Overflow)
        );
        assert_eq!(
            "-1.21"
                .parse::<Value>()
                .unwrap()
                .floor_to(a)
                .unwrap()
                .to_string(),
            "-1.3"
        );
        assert_eq!(Value::ONE.floor_to(Value::ZERO), Err(Error::InvalidStep));
    }
}

//! BBGO dnum's normalized 16-significant-digit coefficient/exponent arithmetic.
//! Finite checked subset: no infinity sentinels, wrapping exponents or silent underflow.
use crate::{Error, Legacy};
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{cmp::Ordering, fmt, str::FromStr};

const MIN: u64 = 1_000_000_000_000_000;
const MAX: u64 = 9_999_999_999_999_999;

/// Value = sign * coefficient * 10^(exponent - 16).
/// Private, canonical fields keep equality, ordering and hashing consistent.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dnum {
    coefficient: u64,
    exponent: i32,
    negative: bool,
}

impl Dnum {
    pub const BACKEND: &'static str = "dnum-16";
    pub const ZERO: Self = Self {
        coefficient: 0,
        exponent: 0,
        negative: false,
    };
    pub const ONE: Self = Self {
        coefficient: MIN,
        exponent: 1,
        negative: false,
    };

    pub fn coefficient(self) -> u64 {
        self.coefficient
    }
    pub fn exponent(self) -> i32 {
        self.exponent
    }
    /// Wire/accounting inputs must not silently discard significant digits.
    /// `FromStr` separately preserves Go dnum's truncating input behavior.
    pub fn parse_exact(text: &str) -> Result<Self, Error> {
        let value: Self = text.parse()?;
        let mantissa = text
            .trim_start_matches(['+', '-'])
            .split(['e', 'E', '%'])
            .next()
            .unwrap_or("");
        let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
        if digits.trim_start_matches('0').trim_end_matches('0').len() > 16 {
            return Err(Error::Precision);
        }
        Ok(value)
    }

    fn normalize(negative: bool, mut coefficient: u128, mut exponent: i32) -> Result<Self, Error> {
        if coefficient == 0 {
            return Ok(Self::ZERO);
        }
        // BBGO New rounds each discarded digit half up, not banker's rounding.
        while coefficient > u128::from(MAX) {
            coefficient = (coefficient + 5) / 10;
            exponent = exponent.checked_add(1).ok_or(Error::Overflow)?;
        }
        while coefficient < u128::from(MIN) {
            coefficient *= 10;
            exponent = exponent.checked_sub(1).ok_or(Error::Overflow)?;
        }
        if !(-128..=127).contains(&exponent) {
            return Err(Error::Overflow);
        }
        Ok(Self {
            coefficient: coefficient as u64,
            exponent,
            negative,
        })
    }
    pub fn from_integer(value: i64) -> Result<Self, Error> {
        Self::normalize(value < 0, u128::from(value.unsigned_abs()), 16)
    }
    pub fn checked_add(self, rhs: Self) -> Result<Self, Error> {
        if self == Self::ZERO {
            return Ok(rhs);
        }
        if rhs == Self::ZERO {
            return Ok(self);
        }
        let (x, y) = if self.exponent >= rhs.exponent {
            (self, rhs)
        } else {
            (rhs, self)
        };
        let shift = (x.exponent - y.exponent) as u32;
        if shift > 15 {
            return Ok(x);
        }
        // Match Go align: truncate the smaller coefficient BEFORE signed addition.
        let small = i128::from(y.coefficient / 10_u64.pow(shift));
        let a = i128::from(x.coefficient) * if x.negative { -1 } else { 1 };
        let b = small * if y.negative { -1 } else { 1 };
        let sum = a + b;
        Self::normalize(sum < 0, sum.unsigned_abs(), x.exponent)
    }
    pub fn checked_sub(self, mut rhs: Self) -> Result<Self, Error> {
        if rhs != Self::ZERO {
            rhs.negative = !rhs.negative;
        }
        self.checked_add(rhs)
    }
    pub fn checked_mul(self, rhs: Self) -> Result<Self, Error> {
        // Port Go's uneven split, including its intentional omission of low*low.
        let (xh, xl) = (self.coefficient / 10_000_000, self.coefficient % 10_000_000);
        let (yh, yl) = (rhs.coefficient / 10_000_000, rhs.coefficient % 10_000_000);
        let coefficient = u128::from(xh) * u128::from(yh)
            + (u128::from(xl) * u128::from(yh) + u128::from(yl) * u128::from(xh)) / 10_000_000;
        Self::normalize(
            self.negative != rhs.negative,
            coefficient,
            self.exponent + rhs.exponent - 2,
        )
    }
    pub fn checked_div(self, rhs: Self) -> Result<Self, Error> {
        if rhs == Self::ZERO {
            return Err(Error::DivisionByZero);
        }
        let coefficient =
            u128::from(self.coefficient) * 10_u128.pow(16) / u128::from(rhs.coefficient);
        Self::normalize(
            self.negative != rhs.negative,
            coefficient,
            self.exponent - rhs.exponent,
        )
    }
    // Exact integer intermediates for accounting extensions, not Go's rounded Mul/Div.
    fn scaled_integer(self, power: i32) -> BigInt {
        let value = BigInt::from(self.coefficient) * pow10((self.exponent - 16 - power) as u32);
        if self.negative { -value } else { value }
    }
    fn from_scaled_integer(
        value: BigInt,
        power: i32,
        ceil: bool,
        exact: bool,
    ) -> Result<Self, Error> {
        if value.is_zero() {
            return Ok(Self::ZERO);
        }
        let negative = value.is_negative();
        let mut coefficient = value.abs();
        let mut exponent = power + 16;
        let max = BigInt::from(MAX);
        while coefficient > max {
            let remainder = &coefficient % 10_u8;
            if exact && !remainder.is_zero() {
                return Err(Error::Precision);
            }
            coefficient /= 10_u8;
            if ceil && !remainder.is_zero() {
                coefficient += 1_u8;
            }
            exponent += 1;
        }
        Self::normalize(
            negative,
            coefficient.to_u128().ok_or(Error::Overflow)?,
            exponent,
        )
    }
    /// Conservative nonnegative product rounded UP at 16 significant digits.
    pub fn checked_mul_ceil(self, rhs: Self) -> Result<Self, Error> {
        if self.negative || rhs.negative {
            return Err(Error::Negative);
        }
        let product = BigInt::from(self.coefficient) * BigInt::from(rhs.coefficient);
        Self::from_scaled_integer(product, self.exponent + rhs.exponent - 32, true, false)
    }
    /// Single-round allocation toward zero at 16 significant digits.
    pub fn checked_mul_div(self, rhs: Self, divisor: Self) -> Result<Self, Error> {
        if divisor == Self::ZERO {
            return Err(Error::DivisionByZero);
        }
        let numerator = BigInt::from(self.coefficient) * BigInt::from(rhs.coefficient) * pow10(32);
        let coefficient = numerator / BigInt::from(divisor.coefficient);
        let signed = if self.negative ^ rhs.negative ^ divisor.negative {
            -coefficient
        } else {
            coefficient
        };
        Self::from_scaled_integer(
            signed,
            self.exponent + rhs.exponent - divisor.exponent - 48,
            false,
            false,
        )
    }
    pub fn floor_to(self, step: Self) -> Result<Self, Error> {
        if step <= Self::ZERO {
            return Err(Error::InvalidStep);
        }
        let power = self.exponent.min(step.exponent) - 16;
        let value = self.scaled_integer(power);
        let divisor = step.scaled_integer(power);
        let mut quotient = &value / &divisor;
        if value.is_negative() && !(&value % &divisor).is_zero() {
            quotient -= 1_u8;
        }
        // An unrepresentable grid point must fail, never round ABOVE the floor.
        Self::from_scaled_integer(quotient * divisor, power, false, true)
    }
    pub fn to_f64(self) -> f64 {
        let value = self.coefficient as f64 * 10_f64.powi(self.exponent - 16);
        if self.negative { -value } else { value }
    }
}
fn pow10(power: u32) -> BigInt {
    BigInt::from(10_u8).pow(power)
}

impl FromStr for Dnum {
    type Err = Error;
    fn from_str(input: &str) -> Result<Self, Error> {
        if input.len() > 8192 {
            return Err(Error::Syntax);
        }
        let (s, percentage) = input
            .strip_suffix('%')
            .map_or((input, false), |s| (s, true));
        let (negative, s) = if let Some(s) = s.strip_prefix('-') {
            (true, s)
        } else {
            (false, s.strip_prefix('+').unwrap_or(s))
        };
        let (mantissa, power) = if let Some((s, e)) = s.split_once(['e', 'E']) {
            if e.is_empty() {
                return Err(Error::Syntax);
            }
            (s, e.parse::<i32>().map_err(|_| Error::Syntax)?)
        } else {
            (s, 0)
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if (whole.is_empty() && fraction.is_empty())
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(Error::Syntax);
        }
        let digits = format!("{whole}{fraction}");
        let significant = digits.trim_start_matches('0');
        if significant.is_empty() {
            return Ok(Self::ZERO);
        }
        let leading = digits.len() - significant.len();
        let exponent = (whole.len() as i32 - leading as i32)
            .checked_add(power)
            .and_then(|e| e.checked_sub(if percentage { 2 } else { 0 }))
            .ok_or(Error::Overflow)?;
        // Go's parser silently maps outside this range to zero/infinity; fail closed.
        if !(-128..=127).contains(&exponent) {
            return Err(Error::Overflow);
        }
        let kept = &significant[..significant.len().min(16)];
        let coefficient =
            kept.parse::<u64>().map_err(|_| Error::Syntax)? * 10_u64.pow(16 - kept.len() as u32);
        Self::normalize(negative, u128::from(coefficient), exponent)
    }
}
impl Ord for Dnum {
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = |x: &Self| {
            if x.coefficient == 0 {
                0
            } else if x.negative {
                -1
            } else {
                1
            }
        };
        let a = sign(self);
        let b = sign(other);
        if a != b {
            return a.cmp(&b);
        }
        let order = (self.exponent, self.coefficient).cmp(&(other.exponent, other.coefficient));
        if a < 0 { order.reverse() } else { order }
    }
}
impl PartialOrd for Dnum {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl fmt::Display for Dnum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::ZERO {
            return f.write_str("0");
        }
        if self.negative {
            f.write_str("-")?;
        }
        let digits = self.coefficient.to_string();
        let digits = digits.trim_end_matches('0');
        if (-19..=0).contains(&self.exponent) {
            write!(f, "0.{}{}", "0".repeat((-self.exponent) as usize), digits)
        } else if self.exponent > 0 && self.exponent <= 16 {
            let position = self.exponent as usize;
            if position >= digits.len() {
                write!(f, "{}{}", digits, "0".repeat(position - digits.len()))
            } else {
                write!(f, "{}.{}", &digits[..position], &digits[position..])
            }
        } else {
            write!(f, "{}", &digits[..1])?;
            if digits.len() > 1 {
                write!(f, ".{}", &digits[1..])?;
            }
            write!(f, "e{}", self.exponent - 1)
        }
    }
}
impl Serialize for Dnum {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Dnum {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse_exact(&String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}
impl TryFrom<Dnum> for Legacy {
    type Error = Error;
    fn try_from(value: Dnum) -> Result<Self, Error> {
        let power = value.exponent - 8;
        let mut raw = BigInt::from(value.coefficient);
        if power >= 0 {
            raw *= pow10(power as u32);
        } else {
            let divisor = pow10((-power) as u32);
            if !(&raw % &divisor).is_zero() {
                return Err(Error::Precision);
            }
            raw /= divisor;
        }
        if value.negative {
            raw = -raw;
        }
        raw.to_i64().map(Self::from_raw).ok_or(Error::Overflow)
    }
}
impl TryFrom<Legacy> for Dnum {
    type Error = Error;
    fn try_from(value: Legacy) -> Result<Self, Error> {
        Self::from_scaled_integer(BigInt::from(value.raw()), -8, false, true)
    }
}

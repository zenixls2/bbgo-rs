//! Statically selected, checked decimal arithmetic.
//!
//! Both concrete backends are always available. `dnum` changes the workspace-facing
//! `Value` alias; generic code uses `Decimal` without boxing or dynamic dispatch.
mod dnum;
mod legacy;

pub use dnum::Dnum;
pub use legacy::Value as Legacy;
pub const SCALE: i64 = 100_000_000;

#[cfg(feature = "dnum")]
pub type Value = Dnum;
#[cfg(not(feature = "dnum"))]
pub type Value = Legacy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid decimal syntax")]
    Syntax,
    #[error("decimal precision cannot be represented exactly")]
    Precision,
    #[error("decimal overflow")]
    Overflow,
    #[error("division by zero")]
    DivisionByZero,
    #[error("step must be positive")]
    InvalidStep,
    #[error("rounded product requires nonnegative operands")]
    Negative,
}

/// Finite decimal arithmetic, statically dispatched and suitable for monomorphization.
/// Rounding depends on the backend; do not interchange saved state without validation.
pub trait Decimal:
    Copy
    + Default
    + Eq
    + Ord
    + std::hash::Hash
    + std::fmt::Debug
    + std::fmt::Display
    + std::str::FromStr<Err = Error>
    + serde::Serialize
    + serde::de::DeserializeOwned
    + Send
    + Sync
    + 'static
{
    const ZERO: Self;
    const ONE: Self;
    const BACKEND: &'static str;
    fn parse_exact(text: &str) -> Result<Self, Error>;
    fn from_integer(value: i64) -> Result<Self, Error>;
    fn checked_add(self, rhs: Self) -> Result<Self, Error>;
    fn checked_sub(self, rhs: Self) -> Result<Self, Error>;
    fn checked_mul(self, rhs: Self) -> Result<Self, Error>;
    fn checked_div(self, rhs: Self) -> Result<Self, Error>;
    fn checked_mul_div(self, rhs: Self, divisor: Self) -> Result<Self, Error>;
    fn checked_mul_ceil(self, rhs: Self) -> Result<Self, Error>;
    fn floor_to(self, step: Self) -> Result<Self, Error>;
    fn to_f64(self) -> f64;
}

macro_rules! decimal_impl {
    ($ty:ty, $name:literal) => {
        impl Decimal for $ty {
            const ZERO: Self = Self::ZERO;
            const ONE: Self = Self::ONE;
            const BACKEND: &'static str = $name;
            fn parse_exact(text: &str) -> Result<Self, Error> {
                Self::parse_exact(text)
            }
            fn from_integer(value: i64) -> Result<Self, Error> {
                Self::from_integer(value)
            }
            fn checked_add(self, rhs: Self) -> Result<Self, Error> {
                self.checked_add(rhs)
            }
            fn checked_sub(self, rhs: Self) -> Result<Self, Error> {
                self.checked_sub(rhs)
            }
            fn checked_mul(self, rhs: Self) -> Result<Self, Error> {
                self.checked_mul(rhs)
            }
            fn checked_div(self, rhs: Self) -> Result<Self, Error> {
                self.checked_div(rhs)
            }
            fn checked_mul_div(self, rhs: Self, divisor: Self) -> Result<Self, Error> {
                self.checked_mul_div(rhs, divisor)
            }
            fn checked_mul_ceil(self, rhs: Self) -> Result<Self, Error> {
                self.checked_mul_ceil(rhs)
            }
            fn floor_to(self, step: Self) -> Result<Self, Error> {
                self.floor_to(step)
            }
            fn to_f64(self) -> f64 {
                self.to_f64()
            }
        }
    };
}
decimal_impl!(Legacy, "legacy-8");
decimal_impl!(Dnum, "dnum-16");

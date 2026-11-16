//! Sequence-checked level-2 book. A sequence gap invalidates the book until a snapshot.
use crate::{Error, Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level {
    pub price: Value,
    pub quantity: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookUpdate {
    pub symbol: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}
#[derive(Debug, Clone)]
pub struct OrderBook {
    symbol: String,
    sequence: Option<u64>,
    bids: BTreeMap<Value, Value>,
    asks: BTreeMap<Value, Value>,
}
impl OrderBook {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            sequence: None,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }
    pub fn sequence(&self) -> Option<u64> {
        self.sequence
    }
    pub fn best_bid(&self) -> Option<Level> {
        self.sequence?;
        self.bids
            .last_key_value()
            .map(|(&price, &quantity)| Level { price, quantity })
    }
    pub fn best_ask(&self) -> Option<Level> {
        self.sequence?;
        self.asks
            .first_key_value()
            .map(|(&price, &quantity)| Level { price, quantity })
    }
    pub fn snapshot(
        &mut self,
        symbol: &str,
        sequence: u64,
        bids: &[Level],
        asks: &[Level],
    ) -> Result<()> {
        if symbol != self.symbol {
            return Err(Error::Invalid("book symbol mismatch".into()));
        }
        let mut next = Self::new(symbol);
        apply_levels(&mut next.bids, bids)?;
        apply_levels(&mut next.asks, asks)?;
        next.check_crossed()?;
        next.sequence = Some(sequence);
        *self = next;
        Ok(())
    }
    /// Duplicates return false. Overlapping ranges may bridge the next sequence.
    /// Invalid updates for this symbol fail closed, including malformed levels.
    pub fn update(&mut self, update: &BookUpdate) -> Result<bool> {
        if update.symbol != self.symbol {
            return Err(Error::Invalid("book symbol mismatch".into()));
        }
        let result = self.apply(update);
        if result.is_err() {
            self.sequence = None;
        }
        result
    }
    fn apply(&mut self, update: &BookUpdate) -> Result<bool> {
        let last = self
            .sequence
            .ok_or_else(|| Error::Invalid("book needs snapshot".into()))?;
        if update.first_sequence > update.last_sequence {
            return Err(Error::Invalid("inverted book sequence range".into()));
        }
        if update.last_sequence <= last {
            return Ok(false);
        }
        let expected = last
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("sequence exhausted".into()))?;
        if update.first_sequence > expected {
            return Err(Error::Invalid(
                "book sequence gap; resnapshot required".into(),
            ));
        }
        let mut next = self.clone();
        apply_levels(&mut next.bids, &update.bids)?;
        apply_levels(&mut next.asks, &update.asks)?;
        next.check_crossed()?;
        next.sequence = Some(update.last_sequence);
        *self = next;
        Ok(true)
    }
    fn check_crossed(&self) -> Result<()> {
        if let (Some((&bid, _)), Some((&ask, _))) =
            (self.bids.last_key_value(), self.asks.first_key_value())
        {
            if bid >= ask {
                return Err(Error::Invalid("locked/crossed book".into()));
            }
        }
        Ok(())
    }
}
fn apply_levels(side: &mut BTreeMap<Value, Value>, levels: &[Level]) -> Result<()> {
    for level in levels {
        if level.price <= Value::ZERO || level.quantity < Value::ZERO {
            return Err(Error::Invalid("invalid book level".into()));
        }
        if level.quantity == Value::ZERO {
            side.remove(&level.price);
        } else {
            side.insert(level.price, level.quantity);
        }
    }
    Ok(())
}

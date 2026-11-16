//! Bounded rolling samples; index zero refers to the newest sample, as in BBGO.
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Series {
    capacity: usize,
    values: VecDeque<f64>,
}
impl Series {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("series capacity must be positive");
        }
        Ok(Self {
            capacity,
            values: VecDeque::new(),
        })
    }
    pub fn push(&mut self, value: f64) -> Result<Option<f64>, &'static str> {
        if !value.is_finite() {
            return Err("non-finite series value");
        }
        let removed = if self.values.len() == self.capacity {
            self.values.pop_front()
        } else {
            None
        };
        self.values.push_back(value);
        Ok(removed)
    }
    pub fn last(&self, offset: usize) -> Option<f64> {
        self.values
            .len()
            .checked_sub(offset.checked_add(1)?)
            .and_then(|index| self.values.get(index))
            .copied()
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn ready(&self) -> bool {
        self.values.len() == self.capacity
    }
    pub fn clear(&mut self) {
        self.values.clear();
    }
    pub fn mean(&self) -> Option<f64> {
        if self.is_empty() {
            return None;
        }
        // Scale before summing to avoid unnecessary overflow.
        let count = self.len() as f64;
        let mean: f64 = self.values.iter().map(|v| v / count).sum();
        mean.is_finite().then_some(mean)
    }
    pub fn min(&self) -> Option<f64> {
        self.values.iter().copied().reduce(f64::min)
    }
    pub fn max(&self) -> Option<f64> {
        self.values.iter().copied().reduce(f64::max)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_newest_first_and_rejected_samples_do_not_mutate() {
        let mut s = Series::new(2).unwrap();
        assert_eq!(s.mean(), None);
        s.push(1.0).unwrap();
        s.push(3.0).unwrap();
        assert!(s.ready());
        assert_eq!(s.mean(), Some(2.0));
        assert_eq!(s.push(5.0), Ok(Some(1.0)));
        assert_eq!(s.last(0), Some(5.0));
        assert_eq!(s.last(1), Some(3.0));
        assert_eq!(s.last(usize::MAX), None);
        assert!(s.push(f64::NAN).is_err());
        assert_eq!((s.min(), s.max()), (Some(3.0), Some(5.0)));
        s.clear();
        assert!(s.is_empty());
    }
}

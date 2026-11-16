//! Bounded, incremental indicators corresponding to BBGO's SMA and EWMA.
use std::collections::VecDeque;
pub mod series;

#[derive(Debug, Clone)]
pub struct Sma {
    window: usize,
    samples: VecDeque<f64>,
    sum: f64,
}
impl Sma {
    pub fn new(window: usize) -> Result<Self, &'static str> {
        if window == 0 {
            return Err("window must be positive");
        }
        Ok(Self {
            window,
            samples: VecDeque::new(),
            sum: 0.0,
        })
    }
    pub fn update(&mut self, value: f64) -> Result<f64, &'static str> {
        if !value.is_finite() {
            return Err("non-finite sample");
        }
        let removed = if self.samples.len() == self.window {
            self.samples[0]
        } else {
            0.0
        };
        let next = (self.sum - removed) + value;
        if !next.is_finite() {
            return Err("sum overflow");
        }
        if self.samples.len() == self.window {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
        self.sum = next;
        Ok(self.sum / self.samples.len() as f64)
    }
    pub fn ready(&self) -> bool {
        self.samples.len() == self.window
    }
    pub fn reset(&mut self) {
        self.samples.clear();
        self.sum = 0.0;
    }
}

#[derive(Debug, Clone)]
pub struct Ewma {
    alpha: f64,
    value: Option<f64>,
}
impl Ewma {
    pub fn new(window: usize) -> Result<Self, &'static str> {
        if window == 0 {
            return Err("window must be positive");
        }
        Ok(Self {
            alpha: 2.0 / (window as f64 + 1.0),
            value: None,
        })
    }
    pub fn update(&mut self, sample: f64) -> Result<f64, &'static str> {
        if !sample.is_finite() {
            return Err("non-finite sample");
        }
        let next = self.value.map_or(sample, |last| {
            self.alpha * sample + (1.0 - self.alpha) * last
        });
        if !next.is_finite() {
            return Err("average overflow");
        }
        self.value = Some(next);
        Ok(next)
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
    pub fn reset(&mut self) {
        self.value = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warmup_window_and_reset() {
        let mut sma = Sma::new(3).unwrap();
        assert_eq!(sma.update(1.0), Ok(1.0));
        assert!(!sma.ready());
        sma.update(2.0).unwrap();
        assert_eq!(sma.update(3.0), Ok(2.0));
        assert!(sma.ready());
        assert!(sma.update(f64::NAN).is_err());
        assert_eq!(sma.update(4.0), Ok(3.0));
        sma.reset();
        assert!(!sma.ready());
        let mut ema = Ewma::new(3).unwrap();
        assert_eq!(ema.update(2.0), Ok(2.0));
        assert_eq!(ema.update(4.0), Ok(3.0));
        assert!(ema.update(f64::INFINITY).is_err());
        assert_eq!(ema.value(), Some(3.0));
    }
}

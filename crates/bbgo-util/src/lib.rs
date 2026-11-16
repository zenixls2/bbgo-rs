//! Ports of BBGO utility primitives; see the migration matrix for intentional differences.
pub mod runtime;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn fnv32(s: &str) -> u32 {
    s.bytes().fold(2_166_136_261, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}
pub fn fnv64(s: &str) -> u64 {
    s.bytes().fold(14_695_981_039_346_656_037, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    })
}

/// Same ASCII masking policy as BBGO, extended safely to Unicode scalar values.
pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return "{empty}".into();
    }
    let chars: Vec<_> = key.chars().collect();
    let h = (chars.len() / 3).min(5);
    chars[..h]
        .iter()
        .chain(std::iter::repeat_n(&'*', chars.len() - 2 * h))
        .chain(chars[chars.len() - h..].iter())
        .collect()
}

/// Split on UTF-8 boundaries using a byte budget. An oversized codepoint is
/// emitted alone (rather than looping forever as the original implementation can).
pub fn split_by_length(s: &str, budget: usize) -> Result<Vec<&str>, &'static str> {
    if budget == 0 {
        return Err("chunk length must be positive");
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < s.len() {
        let mut end = start.saturating_add(budget).min(s.len());
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = start + s[start..].chars().next().unwrap().len_utf8();
        }
        chunks.push(&s[start..end]);
        start = end;
    }
    Ok(chunks)
}

/// Linearizable nonce even when the system clock moves backwards.
#[derive(Debug, Default)]
pub struct MillisecondNonce(AtomicI64);
impl MillisecondNonce {
    pub fn new(current: i64) -> Self {
        Self(AtomicI64::new(current))
    }
    /// Clock-injected form for deterministic replay and tests. Units: microseconds,
    /// with wall-clock precision of one millisecond, matching Go's generator.
    pub fn next_at(&self, unix_millis: i64) -> Option<i64> {
        let wall = unix_millis.checked_mul(1000)?;
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                old.checked_add(1).map(|next| next.max(wall))
            })
            .ok()
            .and_then(|old| old.checked_add(1).map(|next| next.max(wall)))
    }
    pub fn next(&self) -> Option<i64> {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis();
        self.next_at(i64::try_from(ms).ok()?)
    }
}

/// Reset and execution are serialized. A panicking callback poisons the primitive;
/// callers must handle that explicitly instead of silently retrying side effects.
#[derive(Debug, Default)]
pub struct Reonce(Mutex<bool>);
impl Reonce {
    pub fn reset(&self) -> Result<(), &'static str> {
        *self.0.lock().map_err(|_| "Reonce poisoned")? = false;
        Ok(())
    }
    pub fn call(&self, f: impl FnOnce()) -> Result<bool, &'static str> {
        let mut done = self.0.lock().map_err(|_| "Reonce poisoned")?;
        if *done {
            return Ok(false);
        }
        f();
        *done = true;
        Ok(true)
    }
}

/// Bounded deterministic exponential delay; the caller owns sleeping/cancellation.
#[derive(Debug, Clone)]
pub struct Backoff {
    initial: Duration,
    maximum: Duration,
    next: Duration,
}
impl Backoff {
    pub fn new(initial: Duration, maximum: Duration) -> Result<Self, &'static str> {
        if initial.is_zero() || initial > maximum {
            return Err("invalid backoff bounds");
        }
        Ok(Self {
            initial,
            maximum,
            next: initial,
        })
    }
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.maximum);
        delay
    }
    pub fn reset(&mut self) {
        self.next = self.initial;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_compatible_hashes_and_masking() {
        assert_eq!(fnv32("hello"), 0x4f9f2cab);
        assert_eq!(fnv64("hello"), 0xa430d84680aabd0b);
        assert_eq!(mask_key(""), "{empty}");
        assert_eq!(mask_key("abcdef"), "ab**ef");
        assert_eq!(mask_key("秘密鍵"), "秘*鍵");
        assert_eq!(split_by_length("a秘密b", 2).unwrap().concat(), "a秘密b");
        assert!(split_by_length("a", 0).is_err());
    }
    #[test]
    fn nonce_is_unique_under_contention_and_clock_rollback() {
        let n = MillisecondNonce::new(0);
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        (0..1000)
                            .map(|_| n.next_at(100).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut values: Vec<_> = handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect();
            values.sort_unstable();
            values.dedup();
            assert_eq!(values.len(), 8000);
        });
        assert!(n.next_at(1).unwrap() > 100_000);
        assert_eq!(MillisecondNonce::new(i64::MAX).next_at(0), None);
    }
    #[test]
    fn resettable_once_and_backoff() {
        let once = Reonce::default();
        assert_eq!(once.call(|| {}), Ok(true));
        assert_eq!(once.call(|| panic!("not called")), Ok(false));
        once.reset().unwrap();
        assert_eq!(once.call(|| {}), Ok(true));
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
        assert_eq!(
            (0..4).map(|_| b.next_delay().as_secs()).collect::<Vec<_>>(),
            [1, 2, 3, 3]
        );
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 1);
    }
}

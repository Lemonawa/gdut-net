use std::time::Duration;

pub const AUTH_FAIL_DELAY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            attempt: 0,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.min.saturating_mul(1u32 << self.attempt.min(20));
        self.attempt = self.attempt.saturating_add(1);
        d.min(self.max).max(self.min)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_then_caps() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
        assert_eq!(b.next_delay(), Duration::from_secs(4));
        let mut last = Duration::ZERO;
        for _ in 0..20 {
            last = b.next_delay();
        }
        assert_eq!(last, Duration::from_secs(300));
    }

    #[test]
    fn reset_restarts() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        b.next_delay();
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}

use std::time::{Duration, Instant};

/// A wall step larger than this against the monotonic clock is a sleep, suspend or clock change.
pub const CLOCK_JUMP: Duration = Duration::from_secs(30);

const BACKOFF: [Duration; 5] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
];

pub trait Clock: Send + Sync {
    /// Time since an arbitrary origin; never goes backwards and pauses while the machine sleeps.
    fn monotonic(&self) -> Duration;
    fn wall_ms(&self) -> i64;
}

pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn wall_ms(&self) -> i64 {
        crate::store::now_ms()
    }
}

/// Delay before retry number `failures` (1-based), capped at 15 minutes.
pub fn backoff(failures: u32) -> Duration {
    let step = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX);
    BACKOFF[step.min(BACKOFF.len() - 1)]
}

/// Detects the wall clock moving apart from the monotonic one between two checks.
pub struct JumpDetector {
    monotonic: Duration,
    wall_ms: i64,
}

impl JumpDetector {
    pub fn new(clock: &dyn Clock) -> Self {
        Self {
            monotonic: clock.monotonic(),
            wall_ms: clock.wall_ms(),
        }
    }

    pub fn jumped(&mut self, clock: &dyn Clock) -> bool {
        let next = Self::new(clock);
        let monotonic_ms =
            i64::try_from((next.monotonic - self.monotonic).as_millis()).unwrap_or(i64::MAX);
        let skew = (next.wall_ms - self.wall_ms).saturating_sub(monotonic_ms);
        *self = next;
        skew.unsigned_abs() > CLOCK_JUMP.as_millis() as u64
    }
}

#[cfg(test)]
pub struct TestClock(std::sync::Mutex<(Duration, i64)>);

#[cfg(test)]
impl TestClock {
    pub fn new() -> Self {
        Self(std::sync::Mutex::new((Duration::ZERO, 1_700_000_000_000)))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap();
        now.0 += by;
        now.1 += i64::try_from(by.as_millis()).unwrap();
    }

    /// Moves only the wall clock, as a suspend or a manual clock change does.
    pub fn jump_wall(&self, by_ms: i64) {
        self.0.lock().unwrap().1 += by_ms;
    }
}

#[cfg(test)]
impl Clock for TestClock {
    fn monotonic(&self) -> Duration {
        self.0.lock().unwrap().0
    }

    fn wall_ms(&self) -> i64 {
        self.0.lock().unwrap().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_tracks_real_time() {
        let clock = SystemClock::new();
        let before = clock.monotonic();
        assert!((clock.wall_ms() - crate::store::now_ms()).abs() < 1000);
        assert!(clock.monotonic() >= before);
        assert!(!JumpDetector::new(&clock).jumped(&clock));
    }

    #[test]
    fn backoff_steps_then_caps() {
        let steps: Vec<_> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(steps, [5, 15, 60, 300, 900, 900, 900]);
        assert_eq!(backoff(0), Duration::from_secs(5));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(900));
    }

    #[test]
    fn jumps_are_wall_steps_beyond_the_monotonic_clock_in_either_direction() {
        let clock = TestClock::new();
        let mut detector = JumpDetector::new(&clock);
        clock.advance(Duration::from_secs(3600));
        assert!(!detector.jumped(&clock));
        clock.jump_wall(30_000);
        assert!(!detector.jumped(&clock));
        clock.jump_wall(30_001);
        assert!(detector.jumped(&clock));
        assert!(!detector.jumped(&clock));
        clock.jump_wall(-8 * 3600 * 1000);
        assert!(detector.jumped(&clock));
    }
}

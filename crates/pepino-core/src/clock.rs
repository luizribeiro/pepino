use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_CLOCK_SKEW_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
}

#[derive(Debug, Default)]
pub struct Clock {
    latest: Option<Hlc>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("hybrid logical clock exhausted")]
pub struct ClockExhausted;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserveOutcome {
    Accepted,
    IgnoredFuture,
}

impl Clock {
    pub const fn new() -> Self {
        Self { latest: None }
    }

    pub const fn from_hlc(latest: Hlc) -> Self {
        Self {
            latest: Some(latest),
        }
    }

    pub const fn latest(&self) -> Option<Hlc> {
        self.latest
    }

    pub fn observe(&mut self, remote: Hlc, wall_ms: u64) -> ObserveOutcome {
        if remote.wall_ms.saturating_sub(wall_ms) > MAX_CLOCK_SKEW_MS {
            return ObserveOutcome::IgnoredFuture;
        }
        self.latest = self.latest.max(Some(remote));
        ObserveOutcome::Accepted
    }

    pub fn tick(&mut self, wall_ms: u64) -> Result<Hlc, ClockExhausted> {
        let next = match self.latest {
            Some(latest) if wall_ms <= latest.wall_ms => match latest.counter.checked_add(1) {
                Some(counter) => Hlc { counter, ..latest },
                None => Hlc {
                    wall_ms: latest.wall_ms.checked_add(1).ok_or(ClockExhausted)?,
                    counter: 0,
                },
            },
            _ => Hlc {
                wall_ms,
                counter: 0,
            },
        };
        self.latest = Some(next);
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn resumes_from_persisted_timestamp() {
        let wall_ms = 1_795_144_800_000;
        let mut running = Clock::new();
        assert_eq!(running.latest(), None);
        let stored = running.tick(wall_ms).unwrap();
        assert_eq!(running.latest(), Some(stored));
        let mut restored = Clock::from_hlc(stored);
        assert_eq!(restored.latest(), Some(stored));
        assert_eq!(restored.tick(wall_ms - 1), running.tick(wall_ms - 1));
        assert_eq!(restored.latest(), running.latest());
    }

    #[test]
    fn rolls_counter_over_within_the_same_wall_millisecond() {
        let wall_ms = 1_795_144_800_000;
        let mut clock = Clock::new();
        assert_eq!(
            clock.tick(wall_ms),
            Ok(Hlc {
                wall_ms,
                counter: 0
            })
        );
        clock.observe(
            Hlc {
                wall_ms,
                counter: u32::MAX - 1,
            },
            wall_ms,
        );
        assert_eq!(
            clock.tick(wall_ms),
            Ok(Hlc {
                wall_ms,
                counter: u32::MAX
            })
        );
        assert_eq!(
            clock.tick(wall_ms),
            Ok(Hlc {
                wall_ms: wall_ms + 1,
                counter: 0
            })
        );
        assert_eq!(
            clock.tick(wall_ms - 1),
            Ok(Hlc {
                wall_ms: wall_ms + 1,
                counter: 1
            })
        );
    }

    #[test]
    fn reports_exhaustion_at_the_maximum_timestamp() {
        let mut clock = Clock::default();
        let maximum = Hlc {
            wall_ms: u64::MAX,
            counter: u32::MAX,
        };
        assert_eq!(clock.observe(maximum, 0), ObserveOutcome::IgnoredFuture);
        assert_eq!(clock.latest(), None);
        assert_eq!(clock.observe(maximum, u64::MAX), ObserveOutcome::Accepted);
        assert_eq!(clock.tick(u64::MAX), Err(ClockExhausted));
        assert_eq!(clock.tick(0), Err(ClockExhausted));
    }

    proptest! {
        #[test]
        fn skew_limit_preserves_local_progress(
            wall_ms in prop_oneof![any::<u64>(), Just(0), (u64::MAX - MAX_CLOCK_SKEW_MS - 1)..=u64::MAX],
            remote_wall_ms in prop_oneof![any::<u64>(), Just(0), Just(u64::MAX)],
            delta in (MAX_CLOCK_SKEW_MS - 1)..=(MAX_CLOCK_SKEW_MS + 1),
            counter in prop_oneof![any::<u32>(), Just(0), Just(u32::MAX)],
        ) {
            let mut clock = Clock::new();
            for remote_wall_ms in [remote_wall_ms, wall_ms.saturating_add(delta)] {
                let remote = Hlc { wall_ms: remote_wall_ms, counter };
                let before = clock.latest();
                let limit = u128::from(wall_ms) + u128::from(MAX_CLOCK_SKEW_MS);
                if u128::from(remote_wall_ms) > limit {
                    prop_assert_eq!(clock.observe(remote, wall_ms), ObserveOutcome::IgnoredFuture);
                    prop_assert_eq!(clock.latest(), before);
                } else {
                    prop_assert_eq!(clock.observe(remote, wall_ms), ObserveOutcome::Accepted);
                    prop_assert_eq!(clock.latest(), before.max(Some(remote)));
                }
            }

            let before = clock.latest();
            let wall_tick = u128::from(wall_ms) << 32;
            let next = before.map_or(wall_tick, |hlc| {
                let packed = (u128::from(hlc.wall_ms) << 32) | u128::from(hlc.counter);
                wall_tick.max(packed + 1)
            });
            if next >= 1 << 96 {
                prop_assert_eq!(clock.tick(wall_ms), Err(ClockExhausted));
                prop_assert_eq!(clock.latest(), before);
            } else {
                let tick = Hlc { wall_ms: (next >> 32) as u64, counter: next as u32 };
                prop_assert_eq!(clock.tick(wall_ms), Ok(tick));
                prop_assert_eq!(clock.latest(), Some(tick));
            }
        }

        #[test]
        fn local_clock_advances_past_all_previous_values(
            steps in prop::collection::vec((0u64..10, prop::option::of((0u64..10, any::<u32>()))), 1..128)
        ) {
            let mut clock = Clock::new();
            let mut seen = None;
            for (wall_ms, remote) in steps {
                if let Some((remote_wall_ms, counter)) = remote {
                    let remote = Hlc { wall_ms: remote_wall_ms, counter };
                    prop_assert_eq!(clock.observe(remote, wall_ms), ObserveOutcome::Accepted);
                    seen = seen.max(Some(remote));
                }
                let local = clock.tick(wall_ms).unwrap();
                prop_assert!(Some(local) > seen);
                prop_assert!(local.wall_ms >= wall_ms);
                seen = Some(local);
            }
        }
    }
}

use std::{
    fmt::Debug,
    thread,
    time::{Duration, Instant},
};

pub const WAIT: Duration = Duration::from_secs(60);

#[track_caller]
pub fn until<T: Debug>(
    mut observe: impl FnMut() -> T,
    condition: impl Fn(&T) -> bool,
    timeout: Duration,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let observed = observe();
        if condition(&observed) {
            return observed;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "condition timed out after {timeout:?}. Last observation: {observed:?}"
        );
        thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

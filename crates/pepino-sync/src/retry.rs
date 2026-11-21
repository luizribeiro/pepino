use std::time::Duration;

const FIRST: Duration = Duration::from_millis(25);
const MAX: Duration = Duration::from_secs(2);

pub(crate) struct Retry(Duration);

impl Retry {
    pub(crate) fn new() -> Self {
        Self(FIRST)
    }

    pub(crate) fn next_delay(&mut self) -> Duration {
        let delay = self.0;
        self.0 = (delay * 2).min(MAX);
        delay
    }

    pub(crate) async fn wait(&mut self) {
        tokio::time::sleep(self.next_delay()).await;
    }
}

#[cfg(test)]
#[tokio::test(start_paused = true)]
async fn doubles_then_caps_the_delay() {
    let mut retry = Retry::new();
    let start = tokio::time::Instant::now();
    retry.wait().await;
    retry.wait().await;
    assert_eq!(start.elapsed(), Duration::from_millis(75));
    for _ in 0..10 {
        retry.next_delay();
    }
    assert_eq!(retry.next_delay(), MAX);
}

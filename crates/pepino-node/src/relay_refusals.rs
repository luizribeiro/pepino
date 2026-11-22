use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use pepino_core::DeviceId;

const WINDOW: Duration = Duration::from_secs(10);
const TRACKED_SENDERS: usize = 1024;

/// Decides which untrusted senders get an answer, so junk cannot make the relay publish at will.
#[derive(Default)]
pub(super) struct Refusals {
    answered: BTreeMap<DeviceId, Instant>,
}

impl Refusals {
    pub(super) fn allow(&mut self, sender: DeviceId, now: Instant) -> bool {
        self.answered
            .retain(|_, at| now.saturating_duration_since(*at) < WINDOW);
        if self.answered.len() >= TRACKED_SENDERS || self.answered.contains_key(&sender) {
            return false;
        }
        self.answered.insert(sender, now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_sender_is_answered_once_per_window() {
        let laptop = DeviceId::from_bytes([31; 32]);
        let phone = DeviceId::from_bytes([47; 32]);
        let start = Instant::now();
        let mut refusals = Refusals::default();
        assert!(refusals.allow(laptop, start));
        assert!(!refusals.allow(laptop, start + WINDOW / 2));
        assert!(refusals.allow(phone, start + WINDOW / 2));
        assert!(refusals.allow(laptop, start + WINDOW));
        assert!(!refusals.allow(phone, start + WINDOW));
    }

    #[test]
    fn a_flood_of_senders_cannot_grow_the_table_or_buy_more_answers() {
        let start = Instant::now();
        let mut refusals = Refusals::default();
        let sender = |index: usize| {
            let mut id = [0; 32];
            id[..8].copy_from_slice(&index.to_be_bytes());
            DeviceId::from_bytes(id)
        };
        let answered = (0..TRACKED_SENDERS * 2)
            .filter(|&index| refusals.allow(sender(index), start))
            .count();
        assert_eq!(answered, TRACKED_SENDERS);
        assert_eq!(refusals.answered.len(), TRACKED_SENDERS);
        assert!(refusals.allow(sender(usize::MAX), start + WINDOW));
        assert_eq!(refusals.answered.len(), 1);
    }
}

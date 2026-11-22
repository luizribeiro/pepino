use std::collections::BTreeMap;

use pepino_core::{Clock, DeviceId, EventId, Hlc, ObserveOutcome};
use serde::{Deserialize, Serialize};

use crate::{AppError, app::invalid};

const MAX_LIVE: usize = 256;
const MAX_TOMBSTONES: usize = 1024;
const MAX_KEY_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 4096;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct Preferences(BTreeMap<String, Preference>);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct Preference {
    pub(crate) hlc: Hlc,
    pub(crate) writer: DeviceId,
    pub(crate) value: Option<String>,
    id: EventId,
}

impl Preference {
    fn new(key: &str, value: Option<String>, writer: DeviceId, hlc: Hlc) -> Self {
        let id = Self::event_id(key, value.as_deref(), writer, hlc);
        Self {
            hlc,
            writer,
            value,
            id,
        }
    }

    fn event_id(key: &str, value: Option<&str>, writer: DeviceId, hlc: Hlc) -> EventId {
        let mut hash = blake3::Hasher::new_derive_key("Pepino identity preference v1");
        hash.update(writer.as_bytes());
        hash.update(&hlc.to_key());
        hash.update(key.as_bytes());
        hash.update(&[0]);
        if let Some(value) = value {
            hash.update(&[1]);
            hash.update(value.as_bytes());
        }
        EventId::from_bytes(*hash.finalize().as_bytes())
    }

    fn acceptable_clock(&self, now: u64) -> bool {
        Clock::new().observe(self.hlc, now) == ObserveOutcome::Accepted
            && crate::Hlc::from(self.hlc).utc().is_some()
    }

    fn revision(&self) -> (Hlc, EventId) {
        (self.hlc, self.id)
    }
}

impl Preferences {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Preference> {
        self.0.get(key)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Preference)> {
        self.0.iter().map(|(key, entry)| (key.as_str(), entry))
    }

    pub(crate) fn write(
        &mut self,
        key: String,
        value: Option<String>,
        writer: DeviceId,
        now: u64,
    ) -> Result<(), AppError> {
        validate(&key, value.as_deref())?;
        if value.is_some()
            && self.get(&key).is_none_or(|entry| entry.value.is_none())
            && self
                .0
                .values()
                .filter(|entry| entry.value.is_some())
                .count()
                >= MAX_LIVE
        {
            return Err(invalid(
                "preference",
                format!("identity preference limit of {MAX_LIVE} live keys reached"),
            ));
        }
        let mut clock = Clock::new();
        let previous = self.get(&key);
        if let Some(entry) = previous {
            clock.observe(entry.hlc, now);
        }
        let clock_error = || {
            invalid(
                "preference",
                previous.map_or_else(
                    || "the local clock is outside the supported range".into(),
                    |entry| format!("check the clock on device {}", entry.writer),
                ),
            )
        };
        let hlc = clock.tick(now).map_err(|_| clock_error())?;
        if crate::Hlc::from(hlc).utc().is_none() || previous.is_some_and(|entry| entry.hlc >= hlc) {
            return Err(clock_error());
        }
        let entry = Preference::new(&key, value, writer, hlc);
        let deleted = entry.value.is_none();
        self.0.insert(key, entry);
        if deleted {
            self.trim_tombstones();
        }
        Ok(())
    }

    pub(crate) fn discard_future(&mut self, now: u64) {
        self.0.retain(|_, entry| entry.acceptable_clock(now));
    }

    pub(crate) fn merge(&mut self, incoming: Self, now: u64) {
        for (key, entry) in incoming.0 {
            let value = entry.value.as_deref();
            if validate(&key, value).is_ok()
                && entry.acceptable_clock(now)
                && entry.id == Preference::event_id(&key, value, entry.writer, entry.hlc)
                && self
                    .get(&key)
                    .is_none_or(|old| old.revision() < entry.revision())
            {
                self.0.insert(key, entry);
            }
        }
        let mut live = 0;
        self.0.retain(|_, entry| {
            live += usize::from(entry.value.is_some());
            entry.value.is_none() || live <= MAX_LIVE
        });
        self.trim_tombstones();
    }

    fn trim_tombstones(&mut self) {
        let mut tombstones: Vec<_> = self
            .0
            .iter()
            .filter(|(_, entry)| entry.value.is_none())
            .map(|(key, entry)| (entry.revision(), key))
            .collect();
        if tombstones.len() > MAX_TOMBSTONES {
            let excess = tombstones.len() - MAX_TOMBSTONES;
            tombstones.select_nth_unstable(excess);
            let expired: Vec<_> = tombstones[..excess]
                .iter()
                .map(|(_, key)| (*key).clone())
                .collect();
            for key in expired {
                self.0.remove(&key);
            }
        }
    }
}

fn validate(key: &str, value: Option<&str>) -> Result<(), AppError> {
    if key.len() > MAX_KEY_BYTES
        || !key.contains('/')
        || !key.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    {
        return Err(invalid(
            "preference",
            format!("expected a namespaced key within {MAX_KEY_BYTES} bytes"),
        ));
    }
    if value.is_some_and(|value| value.len() > MAX_VALUE_BYTES) {
        return Err(invalid(
            "preference",
            format!("value exceeds {MAX_VALUE_BYTES} bytes"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const NOW: u64 = 1_800_000_000_000;

    fn event(key: &str, value: Option<&str>, writer: u8, counter: u32) -> Preferences {
        let hlc = Hlc {
            wall_ms: NOW,
            counter,
        };
        let entry = Preference::new(
            key,
            value.map(str::to_owned),
            DeviceId::from_bytes([writer; 32]),
            hlc,
        );
        Preferences([(key.into(), entry)].into())
    }

    proptest! {
        #[test]
        fn concurrent_writes_and_tombstones_fold_in_any_order(order in any::<[u8; 6]>()) {
            let key = "view/greenhouse";
            let tied = event(key, Some("tag:garden"), 2, 2);
            let deleted = event(key, None, 3, 2);
            let winner = [tied.get(key).unwrap(), deleted.get(key).unwrap()]
                .into_iter().max_by_key(|entry| entry.revision()).unwrap().clone();
            let keys = [
                event(key, Some("tag:tools"), 1, 1),
                tied,
                deleted,
                event("contact/cam", Some("Cam Gardner"), 2, 1),
                event("view/weekend", Some("assignee:me"), 2, 2),
                event("view/weekend", None, 1, 3),
            ];
            let mut expected = Preferences::default();
            for change in &keys {
                expected.merge(change.clone(), NOW);
            }
            prop_assert_eq!(expected.get(key), Some(&winner));
            prop_assert_eq!(expected.get("view/weekend").unwrap().value.as_deref(), None);
            let mut changes: Vec<_> = order.into_iter().zip(keys).collect();
            changes.sort_by_key(|(index, _)| *index);
            let mut merged = Preferences::default();
            for (_, change) in changes {
                merged.merge(change.clone(), NOW);
                merged.merge(change, NOW);
            }
            prop_assert_eq!(merged, expected);
        }
    }

    proptest! {
        #[test]
        fn hostile_remote_hlc_cannot_block_an_honest_write(
            wall_ms in prop_oneof![
                (NOW + pepino_core::MAX_CLOCK_SKEW_MS + 1)..=u64::MAX,
                Just(253_402_300_799_999),
            ],
            counter in prop_oneof![any::<u32>(), Just(u32::MAX)],
        ) {
            let key = "view/weekend";
            let writer = DeviceId::from_bytes([9; 32]);
            let hostile = Preference::new(key, Some("tag:poison".into()), writer, Hlc { wall_ms, counter });
            let mut prefs = Preferences::default();
            prefs.merge(Preferences([(key.into(), hostile)].into()), NOW);
            prop_assert!(prefs.is_empty());
            prefs.write(key.into(), Some("tag:garden".into()), DeviceId::from_bytes([1; 32]), NOW).unwrap();
            prop_assert_eq!(prefs.get(key).unwrap().hlc.wall_ms, NOW);
        }
    }

    #[test]
    fn clock_error_names_the_writer_after_a_local_clock_rollback() {
        let key = "view/weekend";
        let mut prefs = event(key, Some("tag:garden"), 9, 0);
        let writer = DeviceId::from_bytes([9; 32]);
        let error = prefs
            .write(key.into(), None, DeviceId::from_bytes([1; 32]), 0)
            .unwrap_err();
        assert!(error.to_string().contains(&writer.to_string()));
        prefs.discard_future(0);
        assert!(prefs.is_empty());
    }

    #[test]
    fn caps_merged_keys_and_ignores_invalid_entries() {
        let mut prefs = Preferences::default();
        prefs.merge(event("view/greenhouse", Some("tag:garden"), 3, 2), NOW);
        let before = prefs.clone();
        let oversized = "é".repeat(MAX_VALUE_BYTES);
        prefs.merge(event("view/greenhouse", Some(&oversized), 3, 3), NOW);
        prefs.merge(event("greenhouse", Some("tag:tools"), 3, 3), NOW);
        let mut forged = event("view/greenhouse", Some("tag:garden"), 3, 4);
        forged.0.get_mut("view/greenhouse").unwrap().value = Some("tag:tools".into());
        prefs.merge(forged, NOW);
        assert_eq!(prefs, before);
    }
    #[test]
    fn deletions_free_live_slots_and_a_remote_tombstone_does_not_evict_a_view() {
        let mut prefs = Preferences::default();
        let writer = DeviceId::from_bytes([1; 32]);
        for index in 0..MAX_LIVE {
            prefs
                .write(
                    format!("view/old-{index:03}"),
                    Some("tag:tools".into()),
                    writer,
                    NOW,
                )
                .unwrap();
        }
        let full = prefs
            .write("view/zebra".into(), Some("tag:garden".into()), writer, NOW)
            .unwrap_err();
        assert!(full.to_string().contains("256 live keys"));
        for index in 0..MAX_LIVE {
            prefs
                .write(format!("view/old-{index:03}"), None, writer, NOW)
                .unwrap();
        }
        prefs
            .write("view/zebra".into(), Some("tag:garden".into()), writer, NOW)
            .unwrap();
        prefs.merge(event("view/aaa", None, 2, 0), NOW);
        assert_eq!(
            prefs.get("view/zebra").unwrap().value.as_deref(),
            Some("tag:garden")
        );
        prefs
            .write(
                "view/old-000".into(),
                Some("tag:repair".into()),
                writer,
                NOW,
            )
            .unwrap();
    }

    proptest! {
        #[test]
        fn fold_converges_across_both_caps_for_distinct_live_and_deleted_keys(
            ranks in any::<[u16; 3]>(),
            shift in 0u32..10_000,
        ) {
            let snapshots: Vec<_> = (0..3).map(|device| {
                let live = (0..100).map(|i| (format!("view/live-{device}-{i:03}"), Some("tag:garden")));
                let deleted = (0..360).map(|i| (format!("view/deleted-{device}-{i:03}"), None));
                Preferences(live.chain(deleted).enumerate().map(|(i, (key, value))| {
                    let clock = Hlc {wall_ms: NOW, counter: shift + i as u32};
                    let entry = Preference::new(&key, value.map(str::to_owned), DeviceId::from_bytes([device + 1; 32]), clock);
                    (key, entry)
                }).collect())
            }).collect();
            let mut baseline = Preferences::default();
            for snapshot in &snapshots {
                baseline.merge(snapshot.clone(), NOW);
            }
            let mut permuted: Vec<_> = ranks.into_iter().zip(snapshots).collect();
            permuted.sort_by_key(|(rank, _)| *rank);
            let mut folded = Preferences::default();
            for (_, snapshot) in permuted {
                folded.merge(snapshot.clone(), NOW);
                folded.merge(snapshot, NOW);
            }
            prop_assert_eq!(&baseline, &folded);
            prop_assert_eq!(folded.iter().filter(|(_, e)| e.value.is_some()).count(), MAX_LIVE);
            prop_assert_eq!(folded.iter().filter(|(_, e)| e.value.is_none()).count(), MAX_TOMBSTONES);
            prop_assert!(!folded.iter().any(|(_, e)| e.value.is_none() && e.hlc.counter == shift + 100));
        }
    }

    #[test]
    fn rejects_oversized_writes() {
        let mut prefs = Preferences::default();
        let writer = DeviceId::from_bytes([42; 32]);
        let oversized = prefs
            .write(
                "view/greenhouse".into(),
                Some("é".repeat(MAX_VALUE_BYTES)),
                writer,
                NOW,
            )
            .unwrap_err();
        assert!(
            oversized
                .to_string()
                .contains(&format!("{MAX_VALUE_BYTES} bytes"))
        );
        assert!(prefs.write("greenhouse".into(), None, writer, NOW).is_err());
    }
}

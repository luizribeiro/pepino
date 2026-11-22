use std::collections::BTreeMap;

use pepino_core::{DeviceId, EventId, Hlc};
use serde::{Deserialize, Serialize};

use crate::{AppError, app::invalid};

const MAX_ENTRIES: usize = 256;
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
    #[cfg(test)]
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

    pub(crate) fn merge(&mut self, incoming: Self) {
        for (key, entry) in incoming.0 {
            let value = entry.value.as_deref();
            if validate(&key, value).is_ok()
                && crate::Hlc::from(entry.hlc).utc().is_some()
                && entry.id == Preference::event_id(&key, value, entry.writer, entry.hlc)
                && self
                    .get(&key)
                    .is_none_or(|old| old.revision() < entry.revision())
            {
                self.0.insert(key, entry);
                // Keeping the lowest keys makes truncation independent of delivery order.
                if self.0.len() > MAX_ENTRIES {
                    self.0.pop_last();
                }
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

    fn event(key: &str, value: Option<&str>, writer: u8, counter: u32) -> Preferences {
        let hlc = Hlc {
            wall_ms: 1_800_000_000_000,
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
                expected.merge(change.clone());
            }
            prop_assert_eq!(expected.get(key), Some(&winner));
            prop_assert_eq!(expected.get("view/weekend").unwrap().value.as_deref(), None);
            let mut changes: Vec<_> = order.into_iter().zip(keys).collect();
            changes.sort_by_key(|(index, _)| *index);
            let mut merged = Preferences::default();
            for (_, change) in changes {
                merged.merge(change.clone());
                merged.merge(change);
            }
            prop_assert_eq!(merged, expected);
        }
    }

    #[test]
    fn caps_merged_keys_and_ignores_invalid_entries() {
        let mut prefs = Preferences::default();
        for i in 0..MAX_ENTRIES {
            prefs.merge(event(&format!("view/tool-{i:03}"), None, 3, 2));
        }
        let incoming = event("view/greenhouse", Some("tag:garden"), 3, 2);
        let mut opposite = incoming.clone();
        opposite.merge(prefs.clone());
        prefs.merge(incoming);
        assert_eq!(prefs, opposite);
        assert_eq!(prefs.0.len(), MAX_ENTRIES);
        assert!(prefs.get("view/tool-255").is_none());

        let before = prefs.clone();
        let oversized = "é".repeat(MAX_VALUE_BYTES);
        prefs.merge(event("view/greenhouse", Some(&oversized), 3, 3));
        prefs.merge(event("greenhouse", Some("tag:tools"), 3, 3));
        let mut forged = event("view/greenhouse", Some("tag:garden"), 3, 4);
        forged.0.get_mut("view/greenhouse").unwrap().value = Some("tag:tools".into());
        prefs.merge(forged);
        assert_eq!(prefs, before);
    }
}

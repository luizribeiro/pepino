use pepino_core::DeviceId;

use crate::{AppError, LocalState, MissingReference, RosterDevice};

impl LocalState {
    pub fn resolve_device(&self, reference: &str) -> Result<DeviceId, AppError> {
        resolve(&self.roster(), reference)
    }
}

fn resolve(
    roster: &std::collections::BTreeMap<DeviceId, RosterDevice>,
    reference: &str,
) -> Result<DeviceId, AppError> {
    if let Ok(id) = reference.parse::<DeviceId>()
        && roster.contains_key(&id)
    {
        return Ok(id);
    }
    let prefix = reference.to_ascii_lowercase();
    let is_prefix =
        (4..64).contains(&prefix.len()) && prefix.bytes().all(|byte| byte.is_ascii_hexdigit());
    let mut matches = roster.values().filter(|device| {
        device.name == reference || (is_prefix && device.id.to_string().starts_with(&prefix))
    });
    let missing = || AppError::NotFound {
        field: "device".into(),
        reference: MissingReference::Device(reference.into()),
    };
    let first = matches.next().ok_or_else(missing)?;
    if let Some(second) = matches.next() {
        return Err(AppError::AmbiguousDevice {
            reference: reference.into(),
            candidates: [first, second]
                .into_iter()
                .chain(matches)
                .map(|device| device.id.to_string())
                .collect(),
        });
    }
    Ok(first.id)
}

#[cfg(test)]
#[test]
fn names_can_be_shared_and_full_ids_disambiguate_them() {
    let own = DeviceId::from_bytes([11; 32]);
    let other = DeviceId::from_bytes([21; 32]);
    let devices = [own, other]
        .into_iter()
        .map(|id| {
            (
                id,
                RosterDevice::active(id, "Garden tablet".into(), None, None),
            )
        })
        .collect();
    assert_eq!(
        resolve(&devices, "Garden tablet"),
        Err(AppError::AmbiguousDevice {
            reference: "Garden tablet".into(),
            candidates: vec![own.to_string(), other.to_string()],
        })
    );
    assert_eq!(resolve(&devices, &other.to_string()).unwrap(), other);
    assert_eq!(resolve(&devices, "0b0b").unwrap(), own);
    assert!(matches!(
        resolve(&devices, "Workshop"),
        Err(AppError::NotFound { .. })
    ));
}

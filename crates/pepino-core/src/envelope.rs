use std::collections::BTreeSet;

use ciborium::Value;
use ciborium_ll::{Encoder, Header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    DeviceId, Event, EventId, Hlc, InvalidEvent, TaskId, UnknownValue,
    future_value::substitute_unknown_values,
};

const VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    id: EventId,
    author: DeviceId,
    hlc: Hlc,
    event: Event,
}

#[derive(Debug, Error)]
#[error("could not encode event content")]
pub struct EncodeError(#[source] ciborium::ser::Error<std::io::Error>);

#[derive(Debug, Error)]
pub enum NewEnvelopeError {
    #[error(transparent)]
    InvalidEvent(#[from] InvalidEvent),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("invalid event CBOR")]
    Cbor,
    #[error("expected a two-item array with an unsigned format version")]
    InvalidFrame,
    #[error("invalid version 1 event content")]
    InvalidContent,
    #[error("invalid event variant tag")]
    InvalidEventTag,
    #[error("event content is not canonically encoded")]
    NonCanonical,
    #[error("unexpected bytes after event content")]
    TrailingData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeOutcome {
    Known(Envelope),
    UnknownVersion {
        version: u64,
        id: EventId,
        author: Option<DeviceId>,
        hlc: Option<Hlc>,
        bytes: Vec<u8>,
    },
    UnknownEvent {
        variant: String,
        id: EventId,
        author: DeviceId,
        hlc: Hlc,
        bytes: Vec<u8>,
    },
    UnknownValue {
        id: EventId,
        author: DeviceId,
        hlc: Hlc,
        variant: String,
        values: Vec<UnknownValue>,
        bytes: Vec<u8>,
    },
    InvalidEvent {
        id: EventId,
        author: DeviceId,
        hlc: Hlc,
        reason: InvalidEvent,
    },
}

impl DecodeOutcome {
    pub const fn id(&self) -> EventId {
        match self {
            Self::Known(envelope) => envelope.id(),
            Self::UnknownVersion { id, .. }
            | Self::UnknownEvent { id, .. }
            | Self::UnknownValue { id, .. }
            | Self::InvalidEvent { id, .. } => *id,
        }
    }

    pub const fn author(&self) -> Option<DeviceId> {
        match self {
            Self::Known(envelope) => Some(envelope.author()),
            Self::UnknownVersion { author, .. } => *author,
            Self::UnknownEvent { author, .. }
            | Self::UnknownValue { author, .. }
            | Self::InvalidEvent { author, .. } => Some(*author),
        }
    }

    pub const fn hlc(&self) -> Option<Hlc> {
        match self {
            Self::Known(envelope) => Some(envelope.hlc()),
            Self::UnknownVersion { hlc, .. } => *hlc,
            Self::UnknownEvent { hlc, .. }
            | Self::UnknownValue { hlc, .. }
            | Self::InvalidEvent { hlc, .. } => Some(*hlc),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Content<E> {
    author: DeviceId,
    hlc: Hlc,
    event: E,
}

fn encode_content(author: DeviceId, hlc: Hlc, event: &Event) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(&(VERSION, Content { author, hlc, event }), &mut bytes)
        .map_err(EncodeError)?;
    Ok(bytes)
}

impl EventId {
    fn for_content(author: DeviceId, hlc: Hlc, event: &Event) -> Result<Self, EncodeError> {
        Ok(Self::hash_content_bytes(&encode_content(
            author, hlc, event,
        )?))
    }

    fn hash_content_bytes(bytes: &[u8]) -> Self {
        Self::from_bytes(*blake3::hash(bytes).as_bytes())
    }
}

impl Envelope {
    pub fn new(author: DeviceId, hlc: Hlc, event: Event) -> Result<Self, NewEnvelopeError> {
        event.validate()?;
        let id = EventId::for_content(author, hlc, &event)?;
        Ok(Self {
            id,
            author,
            hlc,
            event,
        })
    }

    /// Validates an event and attaches a trusted ID, such as a stored operation hash.
    pub fn with_id(
        id: EventId,
        author: DeviceId,
        hlc: Hlc,
        event: Event,
    ) -> Result<Self, InvalidEvent> {
        event.validate()?;
        Ok(Self {
            id,
            author,
            hlc,
            event,
        })
    }

    pub const fn id(&self) -> EventId {
        self.id
    }

    pub const fn author(&self) -> DeviceId {
        self.author
    }

    pub const fn hlc(&self) -> Hlc {
        self.hlc
    }

    pub const fn event(&self) -> &Event {
        &self.event
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        encode_content(self.author, self.hlc, &self.event)
    }

    pub fn decode(bytes: &[u8]) -> Result<DecodeOutcome, DecodeError> {
        decode_content(bytes, None)
    }

    /// Decodes the content using a trusted ID supplied by the caller.
    /// The supplied ID is not checked against the bytes.
    pub fn decode_with_id(id: EventId, bytes: &[u8]) -> Result<DecodeOutcome, DecodeError> {
        decode_content(bytes, Some(id))
    }

    pub const fn created_task(&self) -> Option<TaskId> {
        match self.event {
            Event::TaskCreated { .. } => Some(TaskId(self.id)),
            _ => None,
        }
    }

    pub const fn order_key(&self) -> (Hlc, EventId) {
        (self.hlc, self.id)
    }
}

fn decode_content(bytes: &[u8], id: Option<EventId>) -> Result<DecodeOutcome, DecodeError> {
    let mut remaining = bytes;
    let frame: Value = ciborium::from_reader(&mut remaining).map_err(|_| DecodeError::Cbor)?;
    if !remaining.is_empty() {
        return Err(DecodeError::TrailingData);
    }
    let Value::Array(frame) = frame else {
        return Err(DecodeError::InvalidFrame);
    };
    let Some(Value::Integer(version)) = frame.first() else {
        return Err(DecodeError::InvalidFrame);
    };
    let version = u64::try_from(*version).map_err(|_| DecodeError::InvalidFrame)?;
    let id = id.unwrap_or_else(|| EventId::hash_content_bytes(bytes));
    if version != VERSION {
        return Ok(DecodeOutcome::UnknownVersion {
            version,
            id,
            author: future_metadata(frame.get(1), "author"),
            hlc: future_metadata(frame.get(1), "hlc"),
            bytes: bytes.to_vec(),
        });
    }
    let [_, content] = frame.as_slice() else {
        return Err(DecodeError::InvalidFrame);
    };

    let Content { author, hlc, event }: Content<Value> = content
        .deserialized()
        .map_err(|_| content_error(content, DecodeError::InvalidContent))?;
    let variant = match &event {
        Value::Text(variant) => variant,
        Value::Map(entries) => match entries.as_slice() {
            [(Value::Text(variant), _)] => variant,
            _ => return Err(content_error(&event, DecodeError::InvalidEventTag)),
        },
        _ => return Err(DecodeError::InvalidEventTag),
    };
    if !Event::knows_variant(variant) {
        return Ok(DecodeOutcome::UnknownEvent {
            variant: variant.clone(),
            id,
            author,
            hlc,
            bytes: bytes.to_vec(),
        });
    }
    let (event, unknown) = match event.deserialized::<Event>() {
        Ok(event) => (event, None),
        Err(_) => {
            let mut substituted = event.clone();
            let values = substitute_unknown_values(&mut substituted);
            if values.is_empty() {
                return Err(content_error(&event, DecodeError::InvalidContent));
            }
            let decoded = substituted
                .deserialized::<Event>()
                .map_err(|_| content_error(&event, DecodeError::InvalidContent))?;
            (decoded, Some((variant.clone(), values, substituted)))
        }
    };
    let canonical = encode_content(author, hlc, &event).map_err(|_| DecodeError::InvalidContent)?;
    let is_canonical = if let Some((_, _, substituted)) = &unknown {
        let mut normalized = frame.clone();
        if let Value::Map(entries) = &mut normalized[1] {
            for (key, value) in entries {
                if key.as_text() == Some("event") {
                    *value = substituted.clone();
                }
            }
        }
        let normalized = Value::Array(normalized);
        let mut remaining = bytes;
        cbor_bytes(&normalized) == canonical
            && compare_known_bytes(&Value::Array(frame), &normalized, &mut remaining).is_some()
            && remaining.is_empty()
    } else {
        canonical == bytes
    };
    if !is_canonical {
        return Err(DecodeError::NonCanonical);
    }
    let envelope = match Envelope::with_id(id, author, hlc, event) {
        Ok(envelope) => envelope,
        Err(reason) => {
            return Ok(DecodeOutcome::InvalidEvent {
                id,
                author,
                hlc,
                reason,
            });
        }
    };
    if let Some((variant, values, _)) = unknown {
        return Ok(DecodeOutcome::UnknownValue {
            id,
            author,
            hlc,
            variant,
            values,
            bytes: bytes.to_vec(),
        });
    }
    Ok(DecodeOutcome::Known(envelope))
}

fn cbor_bytes(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).expect("CBOR values serialize to a vector");
    bytes
}

// Value loses alternate wire encodings such as overlong lengths. Check the
// original bytes as well, treating substituted discriminant slots as opaque.
fn compare_known_bytes(original: &Value, substituted: &Value, bytes: &mut &[u8]) -> Option<()> {
    if original == substituted {
        *bytes = bytes.strip_prefix(cbor_bytes(original).as_slice())?;
        return Some(());
    }
    let mut prefix = Vec::new();
    let mut encoder = Encoder::from(&mut prefix);
    match (original, substituted) {
        (Value::Array(original), Value::Array(substituted)) => {
            encoder.push(Header::Array(Some(original.len()))).ok()?;
            *bytes = bytes.strip_prefix(prefix.as_slice())?;
            for (original, substituted) in original.iter().zip(substituted) {
                compare_known_bytes(original, substituted, bytes)?;
            }
        }
        (Value::Map(original), Value::Map(substituted))
            if original
                .iter()
                .map(|(key, _)| key)
                .eq(substituted.iter().map(|(key, _)| key)) =>
        {
            encoder.push(Header::Map(Some(original.len()))).ok()?;
            *bytes = bytes.strip_prefix(prefix.as_slice())?;
            for ((key, original), (_, substituted)) in original.iter().zip(substituted) {
                *bytes = bytes.strip_prefix(cbor_bytes(key).as_slice())?;
                compare_known_bytes(original, substituted, bytes)?;
            }
        }
        _ => {
            ciborium::from_reader::<Value, _>(bytes).ok()?;
        }
    }
    Some(())
}

fn future_metadata<T: DeserializeOwned>(content: Option<&Value>, name: &str) -> Option<T> {
    let mut matches = content?
        .as_map()?
        .iter()
        .filter(|(key, _)| key.as_text() == Some(name));
    let (_, value) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    value.deserialized().ok()
}

fn content_error(value: &Value, error: DecodeError) -> DecodeError {
    if has_duplicate_fields(value) {
        DecodeError::NonCanonical
    } else {
        error
    }
}

fn has_duplicate_fields(value: &Value) -> bool {
    match value {
        Value::Map(entries) => {
            let mut names = BTreeSet::new();
            entries.iter().any(|(key, value)| {
                matches!(key, Value::Text(name) if !names.insert(name))
                    || has_duplicate_fields(value)
            })
        }
        Value::Array(items) => items.iter().any(has_duplicate_fields),
        Value::Tag(_, value) => has_duplicate_fields(value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_id_tracks_only_signed_content() {
        let author = DeviceId::from_bytes([9; 32]);
        let other_author = DeviceId::from_bytes([10; 32]);
        let hlc = Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 2,
        };
        let task = TaskId(EventId::from_bytes([5; 32]));
        let event = Event::TagAdded {
            task,
            tag: "garden".into(),
        };
        let envelope = Envelope::new(author, hlc, event.clone()).unwrap();
        let external =
            Envelope::with_id(EventId::from_bytes([99; 32]), author, hlc, event.clone()).unwrap();
        let bytes = envelope.encode().unwrap();
        assert_eq!(bytes, external.encode().unwrap());
        assert_eq!(envelope.id.as_bytes(), blake3::hash(&bytes).as_bytes());
        assert_eq!(
            envelope.id,
            EventId::for_content(author, hlc, &event).unwrap()
        );
        assert_ne!(envelope.id, external.id);
        for (author, hlc, event) in [
            (other_author, hlc, event.clone()),
            (
                author,
                Hlc {
                    wall_ms: hlc.wall_ms + 1,
                    ..hlc
                },
                event.clone(),
            ),
            (author, Hlc { counter: 3, ..hlc }, event),
            (
                author,
                hlc,
                Event::TagAdded {
                    task,
                    tag: "repairs".into(),
                },
            ),
            (
                author,
                hlc,
                Event::TagRemoved {
                    task,
                    tag: "garden".into(),
                },
            ),
        ] {
            assert_ne!(
                envelope.id,
                EventId::for_content(author, hlc, &event).unwrap()
            );
        }
    }

    #[test]
    fn order_breaks_clock_ties_with_id() {
        let author = DeviceId::from_bytes([9; 32]);
        let task = TaskId(EventId::from_bytes([5; 32]));
        let early = Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 2,
        };
        let late = Hlc {
            counter: 3,
            ..early
        };
        let make = |byte, hlc| {
            Envelope::with_id(
                EventId::from_bytes([byte; 32]),
                author,
                hlc,
                Event::TaskDeleted { task },
            )
            .unwrap()
        };
        let mut events = [make(1, late), make(9, early), make(3, early)];
        events.sort_by_key(Envelope::order_key);
        assert_eq!(events.map(|event| event.id.as_bytes()[0]), [3, 9, 1]);
    }
}

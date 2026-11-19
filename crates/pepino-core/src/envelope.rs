use ciborium::Value;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{DeviceId, Event, EventId, Hlc};

const VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub id: EventId,
    pub author: DeviceId,
    pub hlc: Hlc,
    pub event: Event,
}

#[derive(Debug, Error)]
#[error("could not encode event content")]
pub struct EncodeError(#[source] ciborium::ser::Error<std::io::Error>);

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
    #[error("unexpected bytes after event content")]
    TrailingData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeOutcome {
    Known(Envelope),
    UnknownVersion { version: u64 },
    UnknownEvent { variant: String },
}

#[derive(Deserialize, Serialize)]
struct Content<E> {
    author: DeviceId,
    hlc: Hlc,
    event: E,
}

pub fn encode_content(author: DeviceId, hlc: Hlc, event: &Event) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(&(VERSION, Content { author, hlc, event }), &mut bytes)
        .map_err(EncodeError)?;
    Ok(bytes)
}

impl EventId {
    pub fn for_content(author: DeviceId, hlc: Hlc, event: &Event) -> Result<Self, EncodeError> {
        Ok(Self::hash_content_bytes(&encode_content(
            author, hlc, event,
        )?))
    }

    fn hash_content_bytes(bytes: &[u8]) -> Self {
        Self::from_bytes(*blake3::hash(bytes).as_bytes())
    }
}

impl Envelope {
    pub fn new(author: DeviceId, hlc: Hlc, event: Event) -> Result<Self, EncodeError> {
        let id = EventId::for_content(author, hlc, &event)?;
        Ok(Self::with_id(id, author, hlc, event))
    }

    pub const fn with_id(id: EventId, author: DeviceId, hlc: Hlc, event: Event) -> Self {
        Self {
            id,
            author,
            hlc,
            event,
        }
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
    let [Value::Integer(version), content] = frame.as_slice() else {
        return Err(DecodeError::InvalidFrame);
    };
    let version = u64::try_from(*version).map_err(|_| DecodeError::InvalidFrame)?;
    if version != VERSION {
        return Ok(DecodeOutcome::UnknownVersion { version });
    }

    let Content { author, hlc, event }: Content<Value> = content
        .deserialized()
        .map_err(|_| DecodeError::InvalidContent)?;
    let variant = match &event {
        Value::Text(variant) => variant,
        Value::Map(entries) => match entries.as_slice() {
            [(Value::Text(variant), _)] => variant,
            _ => return Err(DecodeError::InvalidEventTag),
        },
        _ => return Err(DecodeError::InvalidEventTag),
    };
    if !Event::knows_variant(variant) {
        return Ok(DecodeOutcome::UnknownEvent {
            variant: variant.clone(),
        });
    }
    let event = event
        .deserialized()
        .map_err(|_| DecodeError::InvalidContent)?;
    let id = id.unwrap_or_else(|| EventId::hash_content_bytes(bytes));
    Ok(DecodeOutcome::Known(Envelope::with_id(
        id, author, hlc, event,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskId;

    #[test]
    fn skips_future_events_and_rejects_corrupt_known_events() {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum FutureEvent {
            ReminderAdded { task: TaskId, message: String },
            BoardArchived,
            TagAdded { task: TaskId, tag: u8 },
        }

        let author = DeviceId::from_bytes([61; 32]);
        let hlc = Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 3,
        };
        let task = TaskId(EventId::from_bytes([59; 32]));
        let encode = |event| {
            let mut bytes = Vec::new();
            ciborium::into_writer(&(VERSION, Content { author, hlc, event }), &mut bytes).unwrap();
            bytes
        };
        for (event, variant) in [
            (
                FutureEvent::ReminderAdded {
                    task,
                    message: "Pick up hinges".into(),
                },
                "reminder_added",
            ),
            (FutureEvent::BoardArchived, "board_archived"),
        ] {
            let bytes = encode(event);
            assert_eq!(
                Envelope::decode(&bytes).unwrap(),
                DecodeOutcome::UnknownEvent {
                    variant: variant.into()
                }
            );
            assert!(Envelope::decode(&bytes[..bytes.len() - 1]).is_err());
        }
        assert!(matches!(
            Envelope::decode(&encode(FutureEvent::TagAdded { task, tag: 4 })),
            Err(DecodeError::InvalidContent)
        ));
    }

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
        let external = Envelope::with_id(EventId::from_bytes([99; 32]), author, hlc, event.clone());
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
        };
        let mut events = [make(1, late), make(9, early), make(3, early)];
        events.sort_by_key(Envelope::order_key);
        assert_eq!(events.map(|event| event.id.as_bytes()[0]), [3, 9, 1]);
    }
}

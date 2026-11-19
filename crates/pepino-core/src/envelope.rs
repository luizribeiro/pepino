use serde::Serialize;
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
pub struct EncodeError(#[from] ciborium::ser::Error<std::io::Error>);

#[derive(Serialize)]
struct Content<E> {
    author: DeviceId,
    hlc: Hlc,
    event: E,
}

pub fn encode_content(author: DeviceId, hlc: Hlc, event: &Event) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(&(VERSION, Content { author, hlc, event }), &mut bytes)?;
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

    pub const fn order_key(&self) -> (Hlc, EventId) {
        (self.hlc, self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskId;

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

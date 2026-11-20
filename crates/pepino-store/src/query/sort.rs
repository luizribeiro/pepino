use std::{cmp::Ordering, fmt, str::FromStr};

use pepino_core::Status;
use thiserror::Error;

use crate::TaskRecord;

use super::predicates::folded_chars;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortKey {
    Created,
    Updated,
    Due,
    Priority,
    Status,
    Title,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("unknown sort key {0:?}")]
pub struct InvalidSortKey(pub String);

impl SortKey {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Due => "due",
            Self::Priority => "priority",
            Self::Status => "status",
            Self::Title => "title",
        }
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SortKey {
    type Err = InvalidSortKey;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        [
            Self::Created,
            Self::Updated,
            Self::Due,
            Self::Priority,
            Self::Status,
            Self::Title,
        ]
        .into_iter()
        .find(|key| key.name() == input)
        .ok_or_else(|| InvalidSortKey(input.into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Sort {
    #[default]
    Default,
    By(Vec<(SortKey, Direction)>),
}

impl fmt::Display for Sort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::By(keys) => {
                for (index, (key, direction)) in keys.iter().enumerate() {
                    if index != 0 {
                        f.write_str(",")?;
                    }
                    if *direction == Direction::Descending {
                        f.write_str("-")?;
                    }
                    key.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}

impl FromStr for Sort {
    type Err = InvalidSortKey;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "default" => Ok(Self::Default),
            "" => Ok(Self::By(Vec::new())),
            input => input
                .split(',')
                .map(|part| {
                    let (key, direction) = part
                        .strip_prefix('-')
                        .map_or((part, Direction::Ascending), |key| {
                            (key, Direction::Descending)
                        });
                    Ok((key.parse()?, direction))
                })
                .collect::<Result<_, _>>()
                .map(Self::By),
        }
    }
}

impl Sort {
    /// Compares listing keys, then task ID in ascending order.
    pub fn compare(&self, left: &TaskRecord, right: &TaskRecord) -> Ordering {
        let keys = match self {
            Self::Default => (!left.status.is_open())
                .cmp(&(!right.status.is_open()))
                .then_with(|| SortKey::Priority.compare(Direction::Descending, left, right))
                .then_with(|| SortKey::Due.compare(Direction::Ascending, left, right))
                .then_with(|| SortKey::Created.compare(Direction::Ascending, left, right)),
            Self::By(keys) => keys
                .iter()
                .map(|&(key, direction)| key.compare(direction, left, right))
                .find(|order| order.is_ne())
                .unwrap_or(Ordering::Equal),
        };
        keys.then_with(|| left.id.cmp(&right.id))
    }

    pub fn sort(&self, tasks: &mut [&TaskRecord]) {
        tasks.sort_unstable_by(|left, right| self.compare(left, right));
    }
}

impl SortKey {
    fn compare(self, direction: Direction, left: &TaskRecord, right: &TaskRecord) -> Ordering {
        match self {
            Self::Created => direction.apply(left.created.wall_ms.cmp(&right.created.wall_ms)),
            Self::Updated => direction.apply(left.updated.wall_ms.cmp(&right.updated.wall_ms)),
            Self::Due => direction.compare_optional(left.due, right.due),
            Self::Priority => direction.compare_optional(left.priority, right.priority),
            Self::Status => {
                direction.apply(status_rank(left.status).cmp(&status_rank(right.status)))
            }
            Self::Title => direction.apply(
                folded_chars(&left.title)
                    .cmp(folded_chars(&right.title))
                    .then_with(|| left.title.cmp(&right.title)),
            ),
        }
    }
}

impl Direction {
    fn apply(self, order: Ordering) -> Ordering {
        match self {
            Self::Ascending => order,
            Self::Descending => order.reverse(),
        }
    }

    fn compare_optional<T: Ord>(self, left: Option<T>, right: Option<T>) -> Ordering {
        match (left, right) {
            (Some(left), Some(right)) => self.apply(left.cmp(&right)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
}

fn status_rank(status: Status) -> u8 {
    match status {
        Status::Todo => 0,
        Status::Doing => 1,
        Status::Done => 2,
        Status::Cancelled => 3,
        _ => u8::MAX,
    }
}

#[cfg(test)]
mod tests {
    use serde::{
        Deserialize,
        de::{self, IntoDeserializer},
    };

    use super::*;

    #[test]
    fn each_known_status_has_an_explicit_rank() {
        struct StatusVariants;

        impl<'de> de::Deserializer<'de> for StatusVariants {
            type Error = de::value::Error;

            fn deserialize_any<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
                Err(de::Error::custom(
                    "expected Status to deserialize as an enum",
                ))
            }

            fn deserialize_enum<V: de::Visitor<'de>>(
                self,
                _: &'static str,
                variants: &'static [&'static str],
                visitor: V,
            ) -> Result<V::Value, Self::Error> {
                for (index, name) in variants.iter().enumerate() {
                    let status =
                        Status::deserialize(de::value::StrDeserializer::<Self::Error>::new(name))?;
                    assert_eq!(usize::from(status_rank(status)), index, "{name}");
                }
                visitor.visit_enum("todo".into_deserializer())
            }

            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
                option unit unit_struct newtype_struct seq tuple tuple_struct map struct
                identifier ignored_any
            }
        }

        assert_eq!(Status::deserialize(StatusVariants).unwrap(), Status::Todo);
    }
}

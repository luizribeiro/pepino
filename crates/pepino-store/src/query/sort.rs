use std::cmp::Ordering;

use pepino_core::Status;

use crate::TaskRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortKey {
    Created,
    Updated,
    Due,
    Priority,
    Status,
    Title,
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
            Self::Title => direction.apply(left.title.cmp(&right.title)),
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
        _ => 4,
    }
}

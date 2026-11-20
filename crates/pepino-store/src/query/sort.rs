use std::cmp::Ordering;

use crate::TaskRecord;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sort {
    _private: (),
}

impl Sort {
    /// Compares listing keys, then task ID in ascending order.
    pub fn compare(&self, left: &TaskRecord, right: &TaskRecord) -> Ordering {
        (!left.status.is_open())
            .cmp(&(!right.status.is_open()))
            .then_with(|| compare_optional(left.priority, right.priority, Ordering::reverse))
            .then_with(|| compare_optional(left.due, right.due, |order| order))
            .then_with(|| left.created.wall_ms.cmp(&right.created.wall_ms))
            .then_with(|| left.id.cmp(&right.id))
    }

    pub fn sort(&self, tasks: &mut [&TaskRecord]) {
        tasks.sort_unstable_by(|left, right| self.compare(left, right));
    }
}

fn compare_optional<T: Ord>(
    left: Option<T>,
    right: Option<T>,
    order: impl FnOnce(Ordering) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => order(left.cmp(&right)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

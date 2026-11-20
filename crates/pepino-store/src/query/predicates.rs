use std::cmp::Ordering;

use icu_casemap::CaseMapper;
use pepino_core::{
    Date, FieldValue, Hlc,
    filter::{Comparison, CustomValue, Operator, PresenceField, State},
};

use crate::{BoardView, TaskRecord};

pub(super) fn is(task: &TaskRecord, state: State) -> bool {
    match state {
        State::Blocked => task.blocked,
        State::Ready => task.ready,
        State::Open => task.status.is_open(),
        State::Closed => !task.status.is_open(),
    }
}

pub(super) fn has(task: &TaskRecord, field: &PresenceField) -> bool {
    match field {
        PresenceField::Due => task.due.is_some(),
        PresenceField::Parent => task.parent.is_some(),
        PresenceField::Assignee => !task.assignees.is_empty(),
        PresenceField::Priority => task.priority.is_some(),
        PresenceField::Tag => !task.tags.is_empty(),
        PresenceField::Description => task.description.is_some(),
        PresenceField::Custom(field) => task.custom.contains_key(field),
    }
}

pub(super) fn compare<T: Ord>(actual: Option<T>, alternatives: &[Comparison<T>]) -> bool {
    actual.is_some_and(|actual| {
        alternatives
            .iter()
            .any(|alternative| test(actual.cmp(&alternative.value), alternative.operator))
    })
}

pub(super) fn utc_days(hlc: Hlc, alternatives: &[Comparison<Date>]) -> bool {
    // Day numbers preserve the full unsigned HLC range even beyond Date::MAX.
    let actual = (hlc.wall_ms / 86_400_000) as i64;
    alternatives.iter().any(|alternative| {
        test(
            actual.cmp(&i64::from(alternative.value.to_epoch_days())),
            alternative.operator,
        )
    })
}

pub(super) fn custom(actual: &FieldValue, alternatives: &[Comparison<CustomValue>]) -> bool {
    alternatives.iter().any(|alternative| {
        let ordering = match (actual, &alternative.value) {
            (FieldValue::Integer(left), CustomValue::Integer(right)) => left.cmp(right),
            (FieldValue::Date(left), CustomValue::Date(right)) => left.cmp(right),
            (FieldValue::Text(left), CustomValue::Text(right)) => left.cmp(right),
            _ => return false,
        };
        test(ordering, alternative.operator)
    })
}

fn test(order: Ordering, operator: Operator) -> bool {
    match operator {
        Operator::Eq => order.is_eq(),
        Operator::Lt => order.is_lt(),
        Operator::Le => order.is_le(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => order.is_ge(),
    }
}

pub(super) fn fold(text: &str) -> String {
    let mapper = CaseMapper::new();
    text.chars().map(|ch| mapper.simple_fold(ch)).collect()
}

pub(super) fn folded_text(view: &BoardView, task: &TaskRecord) -> Vec<String> {
    std::iter::once(task.title.as_str())
        .chain(task.description.as_deref())
        .chain(
            view.comments(task.id)
                .iter()
                .map(|comment| comment.text.as_str()),
        )
        .map(fold)
        .collect()
}

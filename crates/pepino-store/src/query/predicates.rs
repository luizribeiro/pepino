use std::cmp::Ordering;

use pepino_core::{
    Date, Hlc,
    filter::{Comparison, Operator},
};
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

fn test(order: Ordering, operator: Operator) -> bool {
    match operator {
        Operator::Eq => order.is_eq(),
        Operator::Lt => order.is_lt(),
        Operator::Le => order.is_le(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => order.is_ge(),
    }
}

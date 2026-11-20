use std::cmp::Ordering;

use pepino_store::query::{Direction::*, Sort, SortKey::*};
use proptest::{prelude::*, sample::subsequence};

use super::fixture::{board, id};

#[test]
fn default_sort_groups_open_tasks_then_uses_priority_due_creation_and_id() {
    let mut tasks: Vec<_> = board().iter().collect();
    Sort::default().sort(&mut tasks);
    let listed: Vec<_> = tasks.iter().map(|task| task.id).collect();
    assert_eq!(
        listed,
        [1, 7, 0, 11, 8, 4, 6, 9, 12, 13, 2, 10, 3, 5].map(id)
    );
}

#[test]
fn each_explicit_key_supports_both_directions_and_keeps_missing_values_last() {
    for (key, first_asc, first_desc, missing_start) in [
        (Created, 1, 13, 14),
        (Updated, 3, 13, 14),
        (Due, 2, 10, 7),
        (Priority, 4, 2, 8),
        (Status, 4, 3, 14),
        (Title, 8, 12, 14),
    ] {
        let mut asc: Vec<_> = board().iter().collect();
        let mut desc = asc.clone();
        assert_eq!(asc.len(), 14);
        Sort::By(vec![(key, Ascending)]).sort(&mut asc);
        Sort::By(vec![(key, Descending)]).sort(&mut desc);
        assert_eq!(asc[0].id, id(first_asc), "{key:?}");
        assert_eq!(desc[0].id, id(first_desc), "{key:?}");
        assert_eq!(&asc[missing_start..], &desc[missing_start..], "{key:?}");
        for task in &desc[missing_start..] {
            match key {
                Due => assert!(task.due.is_none()),
                Priority => assert!(task.priority.is_none()),
                _ => unreachable!(),
            }
        }
    }
}

proptest! {
    #[test]
    fn sort_is_total_and_invariant_under_input_permutation(
        keys in subsequence(vec![Created, Updated, Due, Priority, Status, Title], 0..=6),
        descending in any::<[bool; 6]>(),
        weights in any::<[u32; 14]>(),
    ) {
        let custom = Sort::By(keys.into_iter().zip(descending.map(|desc| {
            if desc { Descending } else { Ascending }
        })).collect());
        for sort in [Sort::default(), custom] {
            let mut listed: Vec<_> = board().iter().collect();
            let mut permuted = listed.clone();
            permuted.sort_by_key(|task| weights[usize::from(task.id.as_bytes()[2] - 1)]);
            sort.sort(&mut listed);
            sort.sort(&mut permuted);
            prop_assert_eq!(&listed, &permuted);
            for (index, left) in listed.iter().enumerate() {
                prop_assert_eq!(sort.compare(left, left), Ordering::Equal);
                for right in &listed[index + 1..] {
                    prop_assert_eq!(sort.compare(left, right), Ordering::Less);
                    prop_assert_eq!(sort.compare(right, left), Ordering::Greater);
                }
            }
        }
    }
}

#[test]
fn sort_keys_parse_and_display_with_cli_names() {
    for (key, name) in [
        (Created, "created"),
        (Updated, "updated"),
        (Due, "due"),
        (Priority, "priority"),
        (Status, "status"),
        (Title, "title"),
    ] {
        assert_eq!(key.to_string(), name);
        assert_eq!(name.parse(), Ok(key));
    }
    assert_eq!(
        "modified"
            .parse::<pepino_store::query::SortKey>()
            .unwrap_err()
            .0,
        "modified"
    );
}

#[test]
fn sort_lists_round_trip_with_descending_markers() {
    for (sort, spec) in [
        (Sort::Default, "default"),
        (Sort::By(vec![]), ""),
        (Sort::By(vec![(Due, Descending)]), "-due"),
        (
            Sort::By(vec![
                (Created, Ascending),
                (Priority, Descending),
                (Title, Ascending),
            ]),
            "created,-priority,title",
        ),
    ] {
        assert_eq!(sort.to_string(), spec);
        assert_eq!(spec.parse(), Ok(sort));
    }
    for invalid in ["--due", "due,", "-default", "title,made-up", "title, due"] {
        assert!(invalid.parse::<Sort>().is_err(), "{invalid}");
    }
}

#[test]
fn title_order_uses_unicode_folding_then_raw_title_and_id() {
    let mut lower = board().task(id(0)).unwrap().clone();
    lower.title = "straße gate".into();
    let mut upper = board().task(id(1)).unwrap().clone();
    upper.title = "STRAẞE GATE".into();
    let mut same = board().task(id(2)).unwrap().clone();
    same.title.clone_from(&upper.title);
    let mut mixed = board().task(id(3)).unwrap().clone();
    mixed.title = "Tighten hinges".into();
    let mut tasks = [&mixed, &lower, &same, &upper];
    let expected_ids = |tasks: [&pepino_store::TaskRecord; 4]| tasks.map(|task| task.id);
    Sort::By(vec![(Title, Ascending)]).sort(&mut tasks);
    assert_eq!(expected_ids(tasks), [same.id, upper.id, lower.id, mixed.id]);
    Sort::By(vec![(Title, Descending)]).sort(&mut tasks);
    assert_eq!(expected_ids(tasks), [mixed.id, lower.id, same.id, upper.id]);
}

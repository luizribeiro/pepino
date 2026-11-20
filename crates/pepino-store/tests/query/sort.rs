use pepino_store::query::{Direction::*, Sort, SortKey::*};

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

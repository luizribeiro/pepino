use pepino_store::query::Sort;

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

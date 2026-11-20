//! In-memory filtering and ordering for a board snapshot.

mod error;
mod eval;
mod predicates;
mod sort;

pub use error::{QueryError, QueryErrorKind};
pub use eval::{Context, PersonResolver, Query, ViewResolver, matches, select};
pub use sort::{Direction, Sort, SortKey};

use pepino_core::filter::{Clause, Filter, Term};

/// Splits out top-level board terms, retaining order, alternatives and negation.
pub fn split_board_terms(filter: &Filter) -> (Vec<Term>, Filter) {
    let (boards, tasks) = filter
        .terms()
        .iter()
        .cloned()
        .partition(|term| matches!(term.clause, Clause::Board(_)));
    (boards, Filter::from_terms(tasks))
}

#[cfg(test)]
mod tests {
    use pepino_core::{Date, filter::parse};

    use super::*;

    #[test]
    fn separates_board_terms_without_changing_either_terms_order() {
        let today = Date::from_ymd_opt(2026, 11, 19).unwrap();
        let filter = parse(
            "board:garden,house tag:hardware -board:archive @weekend is:open",
            today,
        )
        .unwrap();
        let (boards, tasks) = split_board_terms(&filter);
        assert_eq!(
            boards,
            [filter.terms()[0].clone(), filter.terms()[2].clone()]
        );
        assert_eq!(
            tasks,
            parse("tag:hardware @weekend is:open", today).unwrap()
        );
        assert_eq!(split_board_terms(&tasks), (vec![], tasks.clone()));
        assert_eq!(
            split_board_terms(&Filter::from_terms(boards.clone())),
            (boards, Filter::default())
        );
    }
}

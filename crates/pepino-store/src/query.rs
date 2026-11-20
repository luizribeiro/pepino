//! In-memory filtering and ordering for a board snapshot.

mod error;
mod eval;
mod predicates;
mod saved;
mod sort;

pub use error::{QueryError, QueryErrorKind};
pub use eval::{Context, PersonResolver, Prepared, Query, ViewResolver, matches, select};
pub use sort::{Direction, InvalidSortKey, Sort, SortKey};

use pepino_core::filter::{Clause, Filter, Term};

/// Validates that saved references can be expanded without cycles.
pub fn validate_views(filter: &Filter, resolver: &dyn ViewResolver) -> Result<(), QueryError> {
    saved::validate_views(filter, resolver)
}

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
    use pepino_core::filter::parse;

    use super::*;

    #[test]
    fn separates_board_terms_without_changing_either_terms_order() {
        let filter =
            parse("board:garden,house tag:hardware -board:archive @weekend is:open").unwrap();
        let (boards, tasks) = split_board_terms(&filter);
        assert_eq!(
            boards,
            [filter.terms()[0].clone(), filter.terms()[2].clone()]
        );
        assert_eq!(tasks, parse("tag:hardware @weekend is:open").unwrap());
        assert_eq!(split_board_terms(&tasks), (vec![], tasks.clone()));
        assert_eq!(
            split_board_terms(&Filter::from_terms(boards.clone())),
            (boards, Filter::default())
        );
    }
}

#[cfg(test)]
mod graph_tests {
    use std::collections::BTreeMap;

    use super::*;
    use pepino_core::filter::parse;

    #[test]
    fn validating_references_is_independent_of_board_selection() {
        let mut views = BTreeMap::from([("shed", parse("board:Workshop").unwrap())]);
        let filter = parse("-@shed").unwrap();
        assert!(validate_views(&filter, &views).is_ok());
        views.insert("shed", parse("@shed").unwrap());
        assert!(matches!(
            validate_views(&filter, &views).unwrap_err().kind,
            QueryErrorKind::ViewCycle { .. }
        ));
    }
}

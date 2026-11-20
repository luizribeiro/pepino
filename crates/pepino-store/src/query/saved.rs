use std::collections::BTreeMap;

use pepino_core::filter::{Clause, Filter, Term};

#[derive(Clone)]
pub(super) struct Expanded<T> {
    pub(super) value: T,
    pub(super) has_boards: bool,
}

use super::{QueryError, QueryErrorKind, ViewResolver};

pub(super) type IndexedTerm<'a> = (usize, &'a Term);

pub(super) struct SavedViews<'a, T> {
    resolver: &'a dyn ViewResolver,
    enforce_board_rules: bool,
    chain: Vec<String>,
    cache: BTreeMap<String, Expanded<T>>,
}

impl<'a, T: Clone> SavedViews<'a, T> {
    pub(super) fn new(resolver: &'a dyn ViewResolver) -> Self {
        Self {
            resolver,
            enforce_board_rules: true,
            chain: Vec::new(),
            cache: BTreeMap::new(),
        }
    }

    pub(super) fn error(
        &self,
        (term_index, term): IndexedTerm<'_>,
        kind: QueryErrorKind,
    ) -> QueryError {
        QueryError {
            term: term.to_string(),
            term_index,
            kind,
            view_chain: self.chain.clone(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(super) fn expand(
        &mut self,
        term: IndexedTerm<'_>,
        name: &str,
        build: impl FnOnce(&mut Self, &Filter) -> Result<Expanded<T>, QueryError>,
    ) -> Result<Expanded<T>, QueryError> {
        let expanded = match self.cache.get(name) {
            Some(expanded) => expanded.clone(),
            None => {
                let expanded = self.build(term, name, build)?;
                self.cache.insert(name.to_owned(), expanded.clone());
                expanded
            }
        };
        if self.enforce_board_rules && term.1.negated && expanded.has_boards {
            return Err(self.error(
                term,
                QueryErrorKind::NegatedViewWithBoards { name: name.into() },
            ));
        }
        Ok(expanded)
    }

    fn build(
        &mut self,
        term: IndexedTerm<'_>,
        name: &str,
        build: impl FnOnce(&mut Self, &Filter) -> Result<Expanded<T>, QueryError>,
    ) -> Result<Expanded<T>, QueryError> {
        if self.chain.iter().any(|active| active == name) {
            let chain = self
                .chain
                .iter()
                .cloned()
                .chain([name.to_owned()])
                .collect();
            return Err(self.error(term, QueryErrorKind::ViewCycle { chain }));
        }
        let filter = self
            .resolver
            .resolve(name)
            .ok_or_else(|| self.error(term, QueryErrorKind::UnknownView { name: name.into() }))?;
        self.chain.push(name.to_owned());
        let expanded = build(self, &filter);
        self.chain.pop();
        expanded
    }
}

pub(super) fn canonical_terms(filter: &Filter) -> Vec<IndexedTerm<'_>> {
    let mut terms: Vec<_> = filter.terms().iter().enumerate().collect();
    terms.sort_by_cached_key(|(_, term)| term.to_string());
    terms
}

pub(super) fn board_terms(
    filter: &Filter,
    resolver: &dyn ViewResolver,
) -> Result<Vec<Term>, QueryError> {
    let mut boards = Vec::new();
    collect_boards(filter, &mut SavedViews::new(resolver), &mut boards)?;
    Ok(boards)
}

pub(super) fn validate_views(
    filter: &Filter,
    resolver: &dyn ViewResolver,
) -> Result<(), QueryError> {
    let mut saved = SavedViews::new(resolver);
    saved.enforce_board_rules = false;
    collect_boards(filter, &mut saved, &mut Vec::new()).map(|_| ())
}

fn collect_boards(
    filter: &Filter,
    saved: &mut SavedViews<'_, ()>,
    boards: &mut Vec<Term>,
) -> Result<Expanded<()>, QueryError> {
    let mut has_boards = false;
    for indexed @ (_, term) in canonical_terms(filter) {
        match &term.clause {
            Clause::Board(_) => {
                boards.push(term.clone());
                has_boards = true;
            }
            Clause::View(name) => {
                let nested = saved.expand(indexed, name, |saved, filter| {
                    collect_boards(filter, saved, boards)
                })?;
                has_boards |= nested.has_boards;
            }
            _ => {}
        }
    }
    Ok(Expanded {
        value: (),
        has_boards,
    })
}

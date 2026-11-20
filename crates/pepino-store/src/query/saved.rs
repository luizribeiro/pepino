use std::collections::BTreeMap;

use pepino_core::filter::{Clause, Filter, Term};

#[derive(Clone)]
pub(super) struct Expanded<T> {
    pub(super) value: T,
    pub(super) has_boards: bool,
}

use super::{QueryError, QueryErrorKind, ViewResolver};

pub(super) struct SavedViews<'a, T> {
    resolver: &'a dyn ViewResolver,
    chain: Vec<String>,
    cache: BTreeMap<String, Expanded<T>>,
}

impl<'a, T: Clone> SavedViews<'a, T> {
    pub(super) fn new(resolver: &'a dyn ViewResolver) -> Self {
        Self {
            resolver,
            chain: Vec::new(),
            cache: BTreeMap::new(),
        }
    }

    pub(super) fn error(&self, term: &Term, kind: QueryErrorKind) -> QueryError {
        QueryError {
            term: term.to_string(),
            kind,
            view_chain: self.chain.clone(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.cache.len()
    }

    pub(super) fn expand(
        &mut self,
        term: &Term,
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
        if term.negated && expanded.has_boards {
            return Err(self.error(
                term,
                QueryErrorKind::NegatedViewWithBoards { name: name.into() },
            ));
        }
        Ok(expanded)
    }

    fn build(
        &mut self,
        term: &Term,
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

pub(super) fn canonical_terms(filter: &Filter) -> Vec<&Term> {
    let mut terms: Vec<_> = filter.terms().iter().collect();
    terms.sort_by_cached_key(|term| term.to_string());
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

fn collect_boards(
    filter: &Filter,
    saved: &mut SavedViews<'_, ()>,
    boards: &mut Vec<Term>,
) -> Result<Expanded<()>, QueryError> {
    let mut has_boards = false;
    for term in canonical_terms(filter) {
        match &term.clause {
            Clause::Board(_) => {
                boards.push(term.clone());
                has_boards = true;
            }
            Clause::View(name) => {
                let nested = saved.expand(term, name, |saved, filter| {
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

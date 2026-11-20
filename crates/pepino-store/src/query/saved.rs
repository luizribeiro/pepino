use std::collections::BTreeMap;

use pepino_core::filter::{Filter, Term};

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

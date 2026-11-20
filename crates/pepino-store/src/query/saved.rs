use std::collections::BTreeMap;

use pepino_core::filter::{Filter, Term};

use super::{QueryError, QueryErrorKind, ViewResolver};

pub(super) struct SavedViews<'a, T> {
    resolver: &'a dyn ViewResolver,
    chain: Vec<String>,
    cache: BTreeMap<String, T>,
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
        build: impl FnOnce(&mut Self, &Filter) -> Result<T, QueryError>,
    ) -> Result<T, QueryError> {
        if let Some(expanded) = self.cache.get(name) {
            return Ok(expanded.clone());
        }
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
        let expanded = expanded?;
        self.cache.insert(name.to_owned(), expanded.clone());
        Ok(expanded)
    }
}

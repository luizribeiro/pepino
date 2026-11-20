use std::collections::BTreeMap;
use std::fs::File;

use pepino_core::filter::Clause;
use pepino_store::query;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct SavedViews(BTreeMap<String, String>);

impl SavedViews {
    fn referencing(&self, target: &str) -> Vec<String> {
        self.iter()
            .filter_map(|(name, text)| {
                let filter = pepino_core::filter::parse(text, pepino_core::Date::MIN).ok()?;
                filter
                    .terms()
                    .iter()
                    .any(|term| matches!(&term.clause, Clause::View(view) if view == target))
                    .then(|| name.to_owned())
            })
            .collect()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, filter)| (name.as_str(), filter.as_str()))
    }

    pub(crate) fn insert(&mut self, name: String, filter: String) -> bool {
        self.0.insert(name, filter).is_some()
    }

    pub(crate) fn remove(&mut self, name: &str) -> Option<String> {
        self.0.remove(name)
    }
}

use crate::app::{invalid, worker};
use crate::filter::ViewFilters;
use crate::list::query_error;
use crate::{App, AppError, ConflictReference, Date, FilterInput, MissingReference, Response};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SavedView {
    pub name: String,
    pub filter: String,
}

impl App {
    pub(super) async fn reload_state(&mut self) -> Result<(), AppError> {
        let directory = self.directory.clone();
        self.local = worker("reload local state", move || {
            crate::LocalState::load(&directory)
        })
        .await??;
        Ok(())
    }

    async fn persist_views(
        &mut self,
        saved_views: SavedViews,
        guard: File,
    ) -> Result<(), AppError> {
        let mut local = self.local.clone();
        local.saved_views = saved_views;
        let directory = self.directory.clone();
        self.local = worker("persist saved views", move || {
            // The blocking write can outlive the request future.
            let _guard = guard;
            local.replace(&directory)?;
            Ok::<_, AppError>(local)
        })
        .await??;
        Ok(())
    }

    pub(super) async fn save_view(
        &mut self,
        name: String,
        filter: &FilterInput,
        today: Date,
        guard: File,
    ) -> Result<Response, AppError> {
        validate_name(&name)?;
        let parsed_filter = filter.parse(today)?;
        let canonical = parsed_filter.to_string();
        self.reload_state().await?;
        let mut views = self.local.saved_views.clone();
        let replaced = views.insert(name.clone(), canonical.clone());
        let entry = FilterInput::Text(format!("@{name}"));
        let parsed = ViewFilters::new(&views, today);
        parsed.remember(&name, filter.clone(), parsed_filter);
        query::validate_views(&entry.parse(today)?, &|name: &str| parsed.resolve(name))
            .map_err(|error| query_error(error, &entry, &parsed, &self.local.display_name))?;
        self.persist_views(views, guard).await?;
        Ok(Response::ViewSaved {
            view: SavedView {
                name,
                filter: canonical,
            },
            replaced,
        })
    }

    pub(super) async fn delete_view(
        &mut self,
        name: String,
        force: bool,
        guard: File,
    ) -> Result<Response, AppError> {
        validate_name(&name)?;
        self.reload_state().await?;
        let mut views = self.local.saved_views.clone();
        let Some(filter) = views.remove(&name) else {
            return Err(AppError::NotFound {
                field: "name".into(),
                reference: MissingReference::View(name),
            });
        };
        if !force {
            let referenced_by = views.referencing(&name);
            if !referenced_by.is_empty() {
                return Err(AppError::Conflict {
                    field: "name".into(),
                    reference: ConflictReference::View {
                        view: name,
                        referenced_by,
                    },
                });
            }
        }
        self.persist_views(views, guard).await?;
        Ok(Response::ViewDeleted {
            view: SavedView { name, filter },
        })
    }

    pub(super) async fn list_views(&mut self) -> Result<Response, AppError> {
        self.reload_state().await?;
        Ok(Response::Views {
            views: self
                .local
                .saved_views
                .iter()
                .map(|(name, filter)| SavedView {
                    name: name.into(),
                    filter: filter.into(),
                })
                .collect(),
        })
    }
}

fn validate_name(name: &str) -> Result<(), AppError> {
    let mut bytes = name.bytes();
    if name.len() <= 64
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        Ok(())
    } else {
        Err(invalid("name", "expected [a-z][a-z0-9_-]* within 64 bytes"))
    }
}

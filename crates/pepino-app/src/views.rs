use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct SavedViews(BTreeMap<String, String>);

impl SavedViews {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, filter)| (name.as_str(), filter.as_str()))
    }
}

use crate::app::worker;
use crate::{App, AppError, Response};

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

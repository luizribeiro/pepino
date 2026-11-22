use std::collections::BTreeMap;
use std::fs::File;

use pepino_core::filter::Clause;
use pepino_store::query;
use serde::{Deserialize, Serialize};

use crate::app::invalid;
use crate::filter::ViewFilters;
use crate::list::query_error;
use crate::preferences::Preference;
use crate::{
    App, AppError, ConflictReference, FilterInput, LocalState, MissingReference, Response,
};

const VIEW_PREFIX: &str = "view/";
const MAX_NAME_BYTES: usize = 64;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct SavedViews(BTreeMap<String, String>);

impl SavedViews {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn referencing(&self, target: &str) -> Vec<String> {
        self.iter()
            .filter_map(|(name, text)| {
                let filter = pepino_core::filter::parse(text).ok()?;
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SavedView {
    pub name: String,
    pub filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<crate::NamedRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct ViewValue {
    filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovered_as: Option<String>,
}

impl SavedView {
    pub fn new(name: String, filter: String) -> Self {
        Self {
            name,
            filter,
            recovered_as: None,
            updated_by: None,
            updated_at: None,
        }
    }

    /// Selects a definition from a list-views response using the saved-view name rules.
    pub fn find_by_name<'a>(views: &'a [Self], name: &str) -> Result<&'a Self, AppError> {
        validate_name(name)?;
        views
            .iter()
            .find(|view| view.name == name)
            .ok_or_else(|| AppError::NotFound {
                field: "name".into(),
                reference: MissingReference::View(name.into()),
            })
    }
}

impl App {
    pub(super) async fn save_view(
        &mut self,
        name: String,
        filter: &FilterInput,
        now: u64,
        guard: File,
    ) -> Result<Response, AppError> {
        validate_name(&name)?;
        let parsed_filter = filter.parse()?;
        let canonical = parsed_filter.to_string();
        let mut views = self.local.view_filters();
        let replaced = views.insert(name.clone(), canonical.clone());
        let entry = FilterInput::Text(format!("@{name}"));
        let parsed = ViewFilters::new(&views);
        parsed.remember(&name, filter.clone(), parsed_filter);
        query::validate_views(&entry.parse()?, &|name: &str| parsed.resolve(name))
            .map_err(|error| query_error(error, &entry, &parsed, &self.local))?;
        let mut local = self.local.clone();
        local.write_view(
            &name,
            Some(ViewValue {
                filter: canonical.clone(),
                recovered_as: None,
            }),
            now,
        )?;
        self.persist_local(local, guard).await?;
        Ok(Response::ViewSaved {
            view: SavedView::new(name, canonical),
            replaced,
        })
    }

    pub(super) async fn delete_view(
        &mut self,
        name: String,
        force: bool,
        now: u64,
        guard: File,
    ) -> Result<Response, AppError> {
        validate_name(&name)?;
        let mut views = self.local.view_filters();
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
        let mut local = self.local.clone();
        local.write_view(&name, None, now)?;
        self.persist_local(local, guard).await?;
        Ok(Response::ViewDeleted {
            view: SavedView::new(name, filter),
        })
    }

    pub(super) fn list_views(&self) -> Response {
        Response::Views {
            views: self.local.listed_views(),
        }
    }
}

impl LocalState {
    fn view_values(&self) -> impl Iterator<Item = (&str, &Preference, ViewValue)> {
        self.catalog.preferences.iter().filter_map(|(key, entry)| {
            let name = key.strip_prefix(VIEW_PREFIX)?;
            Some((name, entry, decode_view(entry)?))
        })
    }

    fn listed_views(&self) -> Vec<SavedView> {
        let devices = self.devices();
        self.view_values()
            .map(|(name, entry, value)| {
                let mut view = SavedView::new(name.into(), value.filter);
                view.recovered_as = value.recovered_as;
                if entry.writer != self.device_id
                    && self
                        .catalog
                        .written_views
                        .contains(&format!("{VIEW_PREFIX}{name}"))
                {
                    let id = entry.writer.to_string();
                    let name = devices
                        .get(&entry.writer)
                        .cloned()
                        .unwrap_or_else(|| id.clone());
                    view.updated_by = Some(crate::NamedRecord { id, name });
                    view.updated_at = crate::Hlc::from(entry.hlc).utc();
                }
                view
            })
            .collect()
    }

    pub(crate) fn view_filters(&self) -> SavedViews {
        SavedViews(
            self.view_values()
                .map(|(name, _, value)| (name.into(), value.filter))
                .collect(),
        )
    }

    fn write_view(
        &mut self,
        name: &str,
        value: Option<ViewValue>,
        now: u64,
    ) -> Result<(), AppError> {
        let value = value
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| invalid("preference", error))?;
        let key = format!("{VIEW_PREFIX}{name}");
        self.catalog
            .preferences
            .write(key.clone(), value, self.device_id, now)?;
        self.catalog.written_views.insert(key);
        Ok(())
    }

    pub(super) fn migrate_views(&mut self, now: u64) -> Result<(), AppError> {
        let legacy = std::mem::take(&mut self.saved_views);
        for (name, filter) in legacy.iter() {
            validate_name(name)?;
            let entry = self
                .catalog
                .preferences
                .get(&format!("{VIEW_PREFIX}{name}"));
            let (key, recovered_as) = if let Some(entry) = entry {
                if decode_view(entry).is_some_and(|saved| saved.filter == filter) {
                    continue;
                }
                (
                    self.recovery_name(name, &legacy),
                    Some(format!("{name} (from {})", self.device_name())),
                )
            } else {
                (name.into(), None)
            };
            self.write_view(
                &key,
                Some(ViewValue {
                    filter: filter.into(),
                    recovered_as,
                }),
                now,
            )?;
        }
        Ok(())
    }

    fn recovery_name(&self, name: &str, legacy: &SavedViews) -> String {
        let id = self.device_id.to_string();
        for attempt in 1.. {
            let suffix = if attempt == 1 {
                format!("-from-{}", &id[..16])
            } else {
                format!("-from-{}-{attempt}", &id[..16])
            };
            let candidate = format!(
                "{}{suffix}",
                &name[..name.len().min(MAX_NAME_BYTES - suffix.len())]
            );
            if legacy.get(&candidate).is_none()
                && self
                    .catalog
                    .preferences
                    .get(&format!("{VIEW_PREFIX}{candidate}"))
                    .is_none()
            {
                return candidate;
            }
        }
        unreachable!("bounded preference keys guarantee an unused recovery name")
    }
}

fn decode_view(entry: &Preference) -> Option<ViewValue> {
    serde_json::from_str(entry.value.as_deref()?).ok()
}

fn validate_name(name: &str) -> Result<(), AppError> {
    let mut bytes = name.bytes();
    if name.len() <= MAX_NAME_BYTES
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppErrorKind;

    #[tokio::test]
    async fn imports_legacy_views_once_and_keeps_existing_register_entries() {
        use serde_json::json;

        let dir = tempfile::tempdir().unwrap();
        let mut app = App::init_with_options(
            dir.path(),
            10,
            crate::OpenOptions {
                device_name: Some("Garden laptop".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for (name, filter) in [
            ("weekend", "tag:hardware"),
            ("same", "is:open"),
            ("removed", "tag:hardware"),
        ] {
            app.handle(
                crate::Request::Write(crate::WriteRequest::SaveView {
                    name: name.into(),
                    filter: FilterInput::Text(filter.into()),
                }),
                20,
            )
            .await
            .unwrap();
        }
        app.handle(
            crate::Request::Write(crate::WriteRequest::DeleteView {
                name: "removed".into(),
                force: false,
            }),
            21,
        )
        .await
        .unwrap();
        drop(app);

        let path = dir.path().join(crate::state::FILE_NAME);
        let mut local: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        local["saved_views"] = json!({
            "weekend": "tag:garden",
            "same": "is:open",
            "removed": "tag:garden",
            "maintenance": "@weekend",
        });
        std::fs::write(&path, serde_json::to_vec(&local).unwrap()).unwrap();
        let app = App::open(dir.path(), 30).await.unwrap();
        let Response::Views { views } = app.list_views() else {
            panic!("expected list of views");
        };
        let ordinary = |name| {
            SavedView::find_by_name(&views, name)
                .unwrap()
                .filter
                .as_str()
        };
        assert_eq!(ordinary("weekend"), "tag:hardware");
        assert_eq!(ordinary("maintenance"), "@weekend");
        assert_eq!(ordinary("same"), "is:open");
        assert_eq!(views.len(), 5);
        assert!(SavedView::find_by_name(&views, "removed").is_err());
        for name in ["weekend", "removed"] {
            let label = format!("{name} (from Garden laptop)");
            let view = views
                .iter()
                .find(|view| view.recovered_as.as_deref() == Some(label.as_str()))
                .unwrap();
            assert!(view.name.starts_with(&format!("{name}-from-")));
            assert_eq!(view.filter, "tag:garden");
            assert_eq!(app.local.view_filters().get(&view.name), Some("tag:garden"));
        }
        let bytes = std::fs::read(&path).unwrap();
        let migrated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(migrated.get("saved_views").is_none());
        let reopened = App::open(dir.path(), 40).await.unwrap();
        assert_eq!(reopened.list_views(), app.list_views());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn finds_a_saved_definition_or_reports_a_name_error() {
        let views = [
            SavedView::new("repairs".into(), "tag:hardware".into()),
            SavedView::new("weekend".into(), "due:<fri".into()),
        ];
        assert_eq!(
            SavedView::find_by_name(&views, "weekend").unwrap(),
            &views[1]
        );
        assert_eq!(
            SavedView::find_by_name(&views, "wednesday")
                .unwrap_err()
                .kind(),
            AppErrorKind::NotFound
        );
        assert_eq!(
            SavedView::find_by_name(&views, "Weekend Plan")
                .unwrap_err()
                .kind(),
            AppErrorKind::InvalidInput
        );
    }
}

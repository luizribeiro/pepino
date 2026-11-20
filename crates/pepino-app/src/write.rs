use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    EdgeKind, Event, FieldName, FieldValue, IdentityId, InitialFields, InvalidEvent, TaskId,
    filter::Assignee,
};
use pepino_store::{BoardView, TaskRecord};

use crate::app::invalid;
use crate::batch::{Batch, invalid_event};
use crate::projection::{resolve, resolve_id, snapshot_task};
use crate::{App, AppError, ConflictReference, Response, TaskChange, WriteRequest};

impl App {
    pub(super) async fn apply_to_tasks(
        &mut self,
        tasks: Vec<String>,
        change: TaskChange,
        now: u64,
    ) -> Result<Response, AppError> {
        if tasks.is_empty() {
            return Err(invalid("tasks", "expected at least one task"));
        }
        let view = self.board().await?;
        let mut batch = self.batch(now).await?;
        let mut seen = BTreeSet::new();
        let mut order = Vec::new();
        let mut changed_targets = Vec::new();
        for (index, reference) in tasks.into_iter().enumerate() {
            let field = format!("tasks[{index}]");
            let task = resolve_id(
                &view,
                &field,
                &reference,
                matches!(change, TaskChange::Delete),
            )?;
            if !seen.insert(task) {
                continue;
            }
            let before = batch.event_count();
            match &change {
                TaskChange::SetFields { changes } => {
                    set_fields(&mut batch, snapshot_task(&view, task)?, changes.clone())?
                }
                TaskChange::Delete if view.task(task).is_some() => {
                    delete_task(&view, &mut batch, task, &field, &seen)?;
                }
                TaskChange::Delete => {}
                TaskChange::Reopen => {
                    let current = snapshot_task(&view, task)?;
                    if current.status.is_open() {
                        batch.targets.insert(task);
                    } else {
                        set_fields(
                            &mut batch,
                            current,
                            BTreeMap::from([(
                                "status".into(),
                                crate::FieldValue::Status(crate::Status::Todo),
                            )]),
                        )?;
                    }
                }
            }
            order.push(task.to_string());
            if batch.event_count() != before {
                changed_targets.push(task.to_string());
            }
        }
        let mut outcome = self.commit_batch(&view, batch).await?;
        if let Response::Write {
            targets, changed, ..
        } = &mut outcome
        {
            *targets = order;
            if !*changed {
                changed_targets.clear();
            }
        }
        Ok(Response::BatchWrite {
            outcome: Box::new(outcome),
            changed_targets,
        })
    }

    pub(super) fn plan_write(
        &self,
        view: &BoardView,
        batch: &mut Batch,
        request: WriteRequest,
    ) -> Result<(), AppError> {
        match request {
            WriteRequest::SaveView { .. }
            | WriteRequest::DeleteView { .. }
            | WriteRequest::ApplyToTasks { .. } => {
                return Err(invalid("request", "expected a single task write"));
            }

            WriteRequest::AddTask {
                title,
                description,
                status,
                due,
                priority,
                tags,
                assignees,
                fields,
                parent,
                blocks,
                blocked_by,
            } => {
                let fields = InitialFields {
                    description,
                    status: status.into(),
                    due: due.map(Into::into),
                    priority,
                    tags: tags.iter().cloned().collect(),
                    assignees: self.identities(view, assignees)?,
                    custom: custom_fields(fields)?,
                };
                let title = normalize_title(&title, "title")?;
                let creation = Event::TaskCreated { title, fields };
                creation.validate().map_err(|error| {
                    invalid_event(&creation_path(&creation, &tags, &error), error)
                })?;
                let created = TaskId(batch.push(creation, "title")?);
                if let Some(parent) = parent {
                    let to = resolve(view, "parent", &parent)?.id;
                    batch.push(
                        Event::EdgeAdded {
                            from: created,
                            kind: EdgeKind::Parent,
                            to,
                        },
                        "parent",
                    )?;
                }
                for (list, references, reverse) in
                    [("blocks", blocks, false), ("blocked_by", blocked_by, true)]
                {
                    let mut ids = BTreeMap::new();
                    for (index, reference) in references.into_iter().enumerate() {
                        let field = format!("{list}[{index}]");
                        let id = resolve(view, &field, &reference)?.id;
                        ids.entry(id).or_insert(field);
                    }
                    for (target, field) in ids {
                        let (from, to) = if reverse {
                            (target, created)
                        } else {
                            (created, target)
                        };
                        batch.push(
                            Event::EdgeAdded {
                                from,
                                kind: EdgeKind::Blocks,
                                to,
                            },
                            &field,
                        )?;
                    }
                }
            }
            WriteRequest::SetFields { task, changes } => {
                let current = resolve(view, "task", &task)?;
                set_fields(batch, current, changes)?;
            }
            WriteRequest::AddTags { task, tags } => {
                let current = resolve(view, "task", &task)?;
                set_tags(batch, current, tags, true)?;
            }
            WriteRequest::RemoveTags { task, tags } => {
                let current = resolve(view, "task", &task)?;
                set_tags(batch, current, tags, false)?;
            }
            WriteRequest::Assign { task, assignees } => {
                let current = resolve(view, "task", &task)?;
                set_assignees(batch, current, self.identities(view, assignees)?, true)?;
            }
            WriteRequest::Unassign { task, assignees } => {
                let current = resolve(view, "task", &task)?;
                set_assignees(batch, current, self.identities(view, assignees)?, false)?;
            }
            WriteRequest::Link { from, kind, to } => {
                Self::set_edge(view, batch, &from, kind, &to, true)?;
            }
            WriteRequest::Unlink { from, kind, to } => {
                Self::set_edge(view, batch, &from, kind, &to, false)?;
            }
            WriteRequest::Comment { task, text } => {
                let task = resolve(view, "task", &task)?.id;
                if text.trim().is_empty() {
                    return Err(invalid(
                        "text",
                        "comment text must contain a non-whitespace character",
                    ));
                }
                batch.push(Event::CommentAdded { task, text }, "text")?;
            }
            WriteRequest::Delete { task } => {
                let task = resolve_id(view, "task", &task, true)?;
                batch.targets.insert(task);
                if view.task(task).is_some() {
                    delete_task(view, batch, task, "task", &BTreeSet::new())?;
                }
            }
        }
        Ok(())
    }

    fn set_edge(
        view: &BoardView,
        batch: &mut Batch,
        from_ref: &str,
        kind: crate::EdgeKind,
        to_ref: &str,
        present: bool,
    ) -> Result<(), AppError> {
        let kind = kind.into();
        let mut from = resolve_id(view, "from", from_ref, !present)?;
        let mut to = resolve_id(view, "to", to_ref, !present)?;
        if present && from == to {
            let reason = if kind == EdgeKind::Relates {
                format!("task {from} cannot relate to itself")
            } else {
                format!("link would create a cycle through {from}")
            };
            return Err(AppError::Conflict {
                field: "to".into(),
                reference: ConflictReference::Relation { reason },
            });
        }
        if kind == EdgeKind::Relates && from > to {
            std::mem::swap(&mut from, &mut to);
        }
        let edges = view.edges(from);
        let exists = edges.iter().any(|edge| {
            (edge.from, edge.kind, edge.to) == (from, kind, to) && (!present || edge.is_effective())
        });
        if !present && !exists {
            resolve(view, "from", from_ref)?;
            resolve(view, "to", to_ref)?;
        }
        let event = if present {
            Event::EdgeAdded { from, kind, to }
        } else {
            Event::EdgeRemoved { from, kind, to }
        };
        batch.stage(event, "to", exists != present)?;
        if present && !exists && kind == EdgeKind::Parent {
            for edge in edges
                .iter()
                .filter(|edge| edge.from == from && edge.kind == EdgeKind::Parent && edge.to != to)
            {
                batch.push(
                    Event::EdgeRemoved {
                        from,
                        kind,
                        to: edge.to,
                    },
                    "to",
                )?;
            }
        }
        Ok(())
    }

    fn identities(
        &self,
        view: &BoardView,
        references: Vec<String>,
    ) -> Result<BTreeSet<IdentityId>, AppError> {
        let people = |name: &str| self.local.person_candidates(Some(view), name);
        references
            .into_iter()
            .enumerate()
            .map(|(index, reference)| {
                let assignee = if reference == "me" {
                    Assignee::Me
                } else {
                    Assignee::Person(reference)
                };
                pepino_store::query::resolve_person(
                    Some(self.local.identity_id),
                    &people,
                    &assignee,
                )
                .map_err(|error| invalid(format!("assignees[{index}]"), error))
            })
            .collect()
    }
}

fn delete_task(
    view: &BoardView,
    batch: &mut Batch,
    task: TaskId,
    field: &str,
    seen: &BTreeSet<TaskId>,
) -> Result<(), AppError> {
    for edge in view.edges(task) {
        let (from, kind, to) = (edge.from, edge.kind, edge.to);
        let peer = if from == task { to } else { from };
        if !seen.contains(&peer) || view.task(peer).is_none() {
            let direct_target = batch.targets.contains(&peer);
            batch.push(Event::EdgeRemoved { from, kind, to }, field)?;
            if !direct_target {
                batch.targets.remove(&peer);
            }
        }
    }
    batch.push(Event::TaskDeleted { task }, field)?;
    Ok(())
}

fn set_fields(
    batch: &mut Batch,
    current: &TaskRecord,
    changes: BTreeMap<String, crate::FieldValue>,
) -> Result<(), AppError> {
    batch.targets.insert(current.id);
    for (name, value) in changes {
        let path = field_path("changes", &name);
        let value = match value {
            crate::FieldValue::Text(title) if name == "title" => {
                FieldValue::Text(normalize_title(&title, &path)?)
            }
            value => value.into(),
        };
        let field =
            FieldName::new(&name).map_err(|error| invalid(&path, format!("{name:?}: {error}")))?;
        let changed = current_value(current, &field) != value;
        batch.stage(
            Event::TaskFieldSet {
                task: current.id,
                field,
                value,
            },
            &path,
            changed,
        )?;
    }
    Ok(())
}

fn normalize_title(title: &str, path: &str) -> Result<String, AppError> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(invalid(
            path,
            format!("invalid title {title:?}: expected a non-empty title"),
        ));
    }
    if trimmed.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return Err(invalid(
            path,
            format!("invalid title {title:?}: expected a single line"),
        ));
    }
    Ok(trimmed.to_owned())
}

fn current_value(task: &TaskRecord, field: &FieldName) -> FieldValue {
    let value = match field.as_str() {
        "title" => Some(FieldValue::Text(task.title.clone())),
        "description" => task.description.clone().map(FieldValue::Text),
        "status" => Some(FieldValue::Status(task.status)),
        "due" => task.due.map(FieldValue::Date),
        "priority" => task.priority.map(FieldValue::Integer),
        _ => task.custom.get(field).cloned(),
    };
    value.unwrap_or(FieldValue::Unset)
}

fn set_tags(
    batch: &mut Batch,
    current: &TaskRecord,
    tags: Vec<String>,
    present: bool,
) -> Result<(), AppError> {
    let task = current.id;
    batch.targets.insert(task);
    let mut indexed = BTreeMap::new();
    for (index, tag) in tags.into_iter().enumerate() {
        indexed.entry(tag).or_insert(index);
    }
    for (tag, index) in indexed {
        let changed = current.tags.contains(&tag) != present;
        let event = if present {
            Event::TagAdded { task, tag }
        } else {
            Event::TagRemoved { task, tag }
        };
        batch.stage(event, &format!("tags[{index}]"), changed)?;
    }
    Ok(())
}

fn set_assignees(
    batch: &mut Batch,
    current: &TaskRecord,
    assignees: BTreeSet<IdentityId>,
    present: bool,
) -> Result<(), AppError> {
    let task = current.id;
    batch.targets.insert(task);
    for who in assignees {
        let event = if present {
            Event::AssigneeAdded { task, who }
        } else {
            Event::AssigneeRemoved { task, who }
        };
        batch.stage(
            event,
            "assignees",
            current.assignees.contains(&who) != present,
        )?;
    }
    Ok(())
}

fn custom_fields(
    fields: BTreeMap<String, crate::FieldValue>,
) -> Result<BTreeMap<FieldName, FieldValue>, AppError> {
    fields
        .into_iter()
        .map(|(name, value)| {
            let path = field_path("fields", &name);
            let field = FieldName::new(&name)
                .map_err(|error| invalid(&path, format!("{name:?}: {error}")))?;
            if !field.is_custom() {
                return Err(invalid(path, "expected a custom field name"));
            }
            Ok((field, value.into()))
        })
        .collect()
}

fn creation_path(creation: &Event, tags: &[String], error: &InvalidEvent) -> String {
    let Event::TaskCreated { fields, .. } = creation else {
        return "title".into();
    };
    match error {
        InvalidEvent::EmptyTitle | InvalidEvent::TextTooLong { kind: "title", .. } => {
            "title".into()
        }
        InvalidEvent::TextTooLong {
            kind: "description",
            ..
        } => "description".into(),
        InvalidEvent::Tag(tag) => tags
            .iter()
            .position(|value| value == tag)
            .map_or_else(|| "tags".into(), |index| format!("tags[{index}]")),
        InvalidEvent::TextTooLong {
            kind: "field text",
            max_bytes,
        } => fields
            .custom
            .iter()
            .find_map(|(name, value)| match value {
                FieldValue::Text(text) if text.len() > *max_bytes => {
                    Some(field_path("fields", name.as_str()))
                }
                _ => None,
            })
            .unwrap_or_else(|| "fields".into()),
        _ => "fields".into(),
    }
}

fn field_path(object: &str, field: &str) -> String {
    if !field.is_empty()
        && field
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        format!("{object}.{field}")
    } else {
        format!("{object}[{}]", serde_json::Value::String(field.into()))
    }
}

#[cfg(test)]
#[path = "../tests/support/template.rs"]
pub(super) mod fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReadRequest, Request};
    use serde_json::json;

    #[tokio::test]
    async fn a_batch_with_a_later_invalid_target_leaves_the_first_task_live() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = fixture::open(directory.path(), 100).await.unwrap();
        let create =
            serde_json::from_value(json!({"op":"add_task","title":"Paint the shed"})).unwrap();
        let Response::Write {
            created: Some(task),
            ..
        } = app.handle(create, 100).await.unwrap()
        else {
            panic!("expected a task");
        };
        let request = serde_json::from_value(json!({
            "op":"apply_to_tasks", "tasks":[task, "bad"], "change":{"type":"delete"}
        }))
        .unwrap();
        assert!(
            matches!(app.handle(request, 200).await, Err(AppError::InvalidInput { field, .. }) if field == "tasks[1]")
        );
        let response = app
            .handle(Request::Read(ReadRequest::GetTask { task }), 200)
            .await
            .unwrap();
        assert!(matches!(response, Response::Task { .. }));
    }
}

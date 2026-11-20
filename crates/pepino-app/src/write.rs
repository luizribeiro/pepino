use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    EdgeKind, Event, FieldName, FieldValue, IdentityId, InitialFields, InvalidEvent, TaskId,
};
use pepino_store::{BoardView, TaskRecord};

use crate::app::invalid;
use crate::batch::{Batch, invalid_event};
use crate::projection::resolve;
use crate::{App, AppError, WriteRequest};

impl App {
    pub(super) fn plan_write(
        &self,
        view: &BoardView,
        batch: &mut Batch,
        request: WriteRequest,
    ) -> Result<(), AppError> {
        match request {
            WriteRequest::SaveView { .. } | WriteRequest::DeleteView { .. } => {
                unreachable!("saved views are handled before event planning")
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
                    assignees: self.identities(assignees)?,
                    custom: custom_fields(fields)?,
                };
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
                let mut outgoing = BTreeSet::new();
                for (list, references, reverse) in
                    [("blocks", blocks, false), ("blocked_by", blocked_by, true)]
                {
                    let mut ids = BTreeMap::new();
                    for (index, reference) in references.into_iter().enumerate() {
                        let field = format!("{list}[{index}]");
                        let id = resolve(view, &field, &reference)?.id;
                        if reverse && outgoing.contains(&id) {
                            return Err(invalid(field, "task also appears in blocks"));
                        }
                        ids.entry(id).or_insert(field);
                    }
                    if !reverse {
                        outgoing.extend(ids.keys().copied());
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
                set_assignees(batch, current, self.identities(assignees)?, true)?;
            }
            WriteRequest::Unassign { task, assignees } => {
                let current = resolve(view, "task", &task)?;
                set_assignees(batch, current, self.identities(assignees)?, false)?;
            }
            WriteRequest::Link { from, kind, to } => {
                Self::set_edge(view, batch, &from, kind, &to, true)?;
            }
            WriteRequest::Unlink { from, kind, to } => {
                Self::set_edge(view, batch, &from, kind, &to, false)?;
            }
            WriteRequest::Comment { task, text } => {
                let task = resolve(view, "task", &task)?.id;
                batch.push(Event::CommentAdded { task, text }, "text")?;
            }
            WriteRequest::Delete { task } => {
                let task = resolve(view, "task", &task)?.id;
                batch.push(Event::TaskDeleted { task }, "task")?;
            }
        }
        Ok(())
    }

    fn set_edge(
        view: &BoardView,
        batch: &mut Batch,
        from: &str,
        kind: crate::EdgeKind,
        to: &str,
        present: bool,
    ) -> Result<(), AppError> {
        let kind = kind.into();
        let mut from = resolve(view, "from", from)?.id;
        let mut to = resolve(view, "to", to)?.id;
        if kind == EdgeKind::Relates && from > to {
            std::mem::swap(&mut from, &mut to);
        }
        let exists = view
            .edges(from)
            .iter()
            .any(|edge| (edge.from, edge.kind, edge.to) == (from, kind, to));
        let event = if present {
            Event::EdgeAdded { from, kind, to }
        } else {
            Event::EdgeRemoved { from, kind, to }
        };
        batch.stage(event, "to", exists != present)
    }

    fn identities(&self, references: Vec<String>) -> Result<BTreeSet<IdentityId>, AppError> {
        references
            .into_iter()
            .enumerate()
            .map(|(index, reference)| {
                if reference == "me" {
                    Ok(self.local.identity_id)
                } else {
                    reference.parse().map_err(|_| {
                        invalid(
                            format!("assignees[{index}]"),
                            "expected a full hexadecimal identity ID or me",
                        )
                    })
                }
            })
            .collect()
    }
}

fn set_fields(
    batch: &mut Batch,
    current: &TaskRecord,
    changes: BTreeMap<String, crate::FieldValue>,
) -> Result<(), AppError> {
    batch.affected.insert(current.id);
    for (name, value) in changes {
        let value = value.into();
        let path = field_path("changes", &name);
        let field = FieldName::new(&name).map_err(|error| invalid(&path, error))?;
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
    batch.affected.insert(task);
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
    batch.affected.insert(task);
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
            let field = FieldName::new(&name).map_err(|error| invalid(&path, error))?;
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

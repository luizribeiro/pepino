use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{EdgeKind, Response, TaskSummary};
use serde_json::{Map, json};

use super::{ShortIds, edge_reason, single_line};
use crate::error::{CliError, ErrorKind};
use crate::output::{CommandResult, Warning};
use crate::response::ResponseExt;
use crate::terminal::Context;

pub fn write_document(
    context: &Context<'_>,
    response: Response,
    verb: &str,
) -> Result<CommandResult, CliError> {
    let (message, warnings) = write_output(&response, verb)?;
    Ok(
        CommandResult::app_document(context, response, move |_, _| Ok(message))?
            .with_warnings(warnings),
    )
}

pub fn batch_write_document(
    context: &Context<'_>,
    response: Response,
    verb: &str,
) -> Result<CommandResult, CliError> {
    let Response::BatchWrite {
        outcome,
        changed_targets,
    } = response
    else {
        return Err(CliError::new(
            ErrorKind::Failure,
            "Expected the app to return a batch write.",
        ));
    };
    let write = outcome.write()?;
    let ids = ShortIds::new(write.targets.iter().map(String::as_str));
    let changed: BTreeSet<_> = changed_targets.iter().map(String::as_str).collect();
    let tasks: BTreeMap<_, _> = write
        .tasks
        .iter()
        .chain(write.deleted)
        .map(|task| (task.id.as_str(), task))
        .collect();
    let message = write
        .targets
        .iter()
        .map(|id| {
            let task = tasks.get(id.as_str()).ok_or_else(|| {
                CliError::new(
                    ErrorKind::Failure,
                    "The app omitted a task from the batch outcome.",
                )
            })?;
            let label = write_label(changed.contains(id.as_str()), verb);
            Ok(task_write_line(task, &ids, &label))
        })
        .collect::<Result<Vec<_>, CliError>>()?
        .join("\n");
    CommandResult::app_document(context, *outcome, move |_, _| Ok(message))
}

fn task_write_line(task: &TaskSummary, ids: &ShortIds<'_>, label: &str) -> String {
    format!(
        "{label} {}  {}",
        ids.get(&task.id),
        single_line(&task.title)
    )
}

pub fn relation_document(
    context: &Context<'_>,
    response: Response,
    verb: &str,
    (from, kind, to): (&str, EdgeKind, &str),
) -> Result<CommandResult, CliError> {
    let write = response.write()?;
    let endpoint = |reference: &str| {
        let prefix = reference.to_ascii_lowercase();
        write
            .tasks
            .iter()
            .find(|task| task.id.starts_with(&prefix))
            .map(|task| (task.id.as_str(), task.title.as_str()))
            .or_else(|| {
                write
                    .targets
                    .iter()
                    .find(|id| id.starts_with(&prefix))
                    .map(|id| (id.as_str(), "(deleted task)"))
            })
            .ok_or_else(|| {
                CliError::new(
                    ErrorKind::Failure,
                    "The app omitted a linked task from its response.",
                )
            })
    };
    let (from, to) = (endpoint(from)?, endpoint(to)?);
    let ids = ShortIds::new(
        write
            .edges
            .iter()
            .flat_map(|edge| [edge.from.as_str(), edge.to.as_str()])
            .chain([from.0, to.0]),
    );
    let relation = match kind {
        EdgeKind::Blocks => "blocks",
        EdgeKind::Parent => "has parent",
        EdgeKind::Relates => "relates to",
        _ => "links to",
    };
    let label = write_label(*write.changed, verb);
    let message = format!(
        "{label} {} {} {relation} {} {}",
        ids.get(from.0),
        single_line(from.1),
        ids.get(to.0),
        single_line(to.1)
    );
    let (_, warnings) = write_output_with_ids(&response, verb, Some(&ids))?;
    Ok(
        CommandResult::app_document(context, response, move |_, _| Ok(message))?
            .with_warnings(warnings),
    )
}

fn write_label(changed: bool, verb: &str) -> String {
    if changed {
        verb.into()
    } else {
        format!("Already {}", verb.to_lowercase())
    }
}

pub fn write_documents(
    context: &Context<'_>,
    responses: &[Response],
    verb: &str,
) -> Result<CommandResult, CliError> {
    let writes = responses
        .iter()
        .map(ResponseExt::write)
        .collect::<Result<Vec<_>, _>>()?;
    let ids = ShortIds::new(writes.iter().flat_map(|write| {
        write
            .tasks
            .iter()
            .chain(write.deleted)
            .filter(|task| write.targets.is_empty() || write.targets.contains(&task.id))
            .map(|task| task.id.as_str())
            .chain(
                write
                    .edges
                    .iter()
                    .filter(|edge| edge_reason(edge).is_some())
                    .flat_map(|edge| [edge.from.as_str(), edge.to.as_str()]),
            )
    }));
    let mut messages = Vec::new();
    let mut warnings = Vec::new();
    for response in responses {
        let (message, excluded) = write_output_with_ids(response, verb, Some(&ids))?;
        if !message.is_empty() {
            messages.push(message);
        }
        warnings.extend(excluded);
    }
    let result = combine_responses(responses, &writes)?;
    let human = messages.join("\n");
    Ok(
        CommandResult::app_document(context, result, move |_, _| Ok(human))?
            .with_warnings(warnings),
    )
}

pub fn merge_write_responses(responses: &[Response]) -> Result<Response, CliError> {
    let writes = responses
        .iter()
        .map(ResponseExt::write)
        .collect::<Result<Vec<_>, _>>()?;
    combine_responses(responses, &writes)
}

fn combine_responses(
    responses: &[Response],
    writes: &[crate::response::Write<'_>],
) -> Result<Response, CliError> {
    let mut result = responses
        .first()
        .cloned()
        .ok_or_else(|| CliError::new(ErrorKind::Failure, "No task write outcomes to render."))?;
    if let Response::Write {
        changed,
        created,
        targets,
        tasks,
        deleted,
        edges,
        names,
        ..
    } = &mut result
    {
        *changed = writes.iter().any(|write| *write.changed);
        *created = None;
        let mut seen = BTreeSet::new();
        *targets = writes
            .iter()
            .flat_map(|write| write.targets)
            .filter(|id| seen.insert(id.as_str()))
            .cloned()
            .collect();
        let snapshots: BTreeMap<_, _> = writes
            .iter()
            .flat_map(|write| {
                write
                    .tasks
                    .iter()
                    .map(|task| (task, false))
                    .chain(write.deleted.iter().map(|task| (task, true)))
            })
            .map(|(task, removed)| (task.id.as_str(), (task, removed)))
            .collect();
        tasks.clear();
        deleted.clear();
        for (task, removed) in snapshots.into_values() {
            let records = if removed { &mut *deleted } else { &mut *tasks };
            records.push(task.clone());
        }
        *edges = writes
            .iter()
            .flat_map(|write| write.edges.iter().cloned())
            .collect();
        *names = writes
            .iter()
            .flat_map(|write| write.names.clone())
            .collect();
    }
    Ok(result)
}

fn write_output(response: &Response, verb: &str) -> Result<(String, Vec<Warning>), CliError> {
    write_output_with_ids(response, verb, None)
}

fn write_output_with_ids(
    response: &Response,
    verb: &str,
    batch_ids: Option<&ShortIds<'_>>,
) -> Result<(String, Vec<Warning>), CliError> {
    let write = response.write()?;
    let targets: Vec<_> = if !write.targets.is_empty() {
        write
            .tasks
            .iter()
            .chain(write.deleted)
            .filter(|task| write.targets.contains(&task.id))
            .collect()
    } else if let Some(id) = write.created {
        vec![
            write
                .tasks
                .iter()
                .find(|task| &task.id == id)
                .ok_or_else(|| {
                    CliError::new(
                        ErrorKind::Failure,
                        "The app omitted the created task from the write response.",
                    )
                })?,
        ]
    } else if !write.deleted.is_empty() {
        write.deleted.iter().collect()
    } else {
        write.tasks.iter().collect()
    };
    let excluded: Vec<_> = write
        .edges
        .iter()
        .filter_map(|edge| Some((edge, edge_reason(edge)?)))
        .collect();
    let ids = ShortIds::new(
        targets
            .iter()
            .map(|task| &task.id)
            .chain(excluded.iter().flat_map(|(edge, _)| [&edge.from, &edge.to]))
            .map(String::as_str),
    );
    let ids = batch_ids.unwrap_or(&ids);
    let label = write_label(*write.changed, verb);
    let message = targets
        .iter()
        .map(|task| task_write_line(task, ids, &label))
        .collect::<Vec<_>>()
        .join("\n");
    let warnings = excluded
        .into_iter()
        .map(|(edge, reason)| Warning {
            message: {
                let relation = match edge.kind {
                    EdgeKind::Parent => "parent",
                    EdgeKind::Blocks => "blocks",
                    EdgeKind::Relates => "relates",
                    _ => "links",
                };
                let reason = match edge.reason {
                    Some(pepino_app::EdgeExclusionReason::Cycle) => "would create a cycle".into(),
                    _ => format!("({reason})"),
                };
                format!(
                    "recorded, but not in effect: {} {relation} {} {reason}",
                    ids.get(&edge.from),
                    ids.get(&edge.to)
                )
            },
            details: Map::from_iter([
                ("code".into(), json!("edge_not_effective")),
                ("edge".into(), json!(edge)),
            ]),
        })
        .collect();
    Ok((message, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_requires_at_least_one_write_outcome() {
        let error = combine_responses(&[], &[]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failure);
        assert_eq!(error.to_string(), "No task write outcomes to render.");
    }

    #[test]
    fn created_id_uses_only_visible_tasks_for_uniqueness() {
        let id = "97ad97a8";
        let peer = "97ad97b8";
        let task = |id| {
            json!({
                "id":id,"title":"Replace the greenhouse latch","description":null,"status":"todo",
                "due":null,"priority":null,"fields":{},"tags":[],"assignees":[],"author":"6b376b",
                "created":{"wall_ms":1795149900000u64,"counter":2},"updated":{"wall_ms":1795149900000u64,"counter":2},
                "parent":null,"children":[],"blocks":[],"blocked_by":[],"relates":[],"blocked":false,"ready":true,"comment_count":0
            })
        };
        let mut response = json!({"kind":"write","created":id,"changed":true,"tasks":[task(id),task(peer)],"edges":[]});
        let render =
            |value| write_output(&serde_json::from_value(value).unwrap(), "Created").unwrap();
        assert_eq!(
            render(response.clone()).0,
            "Created 97ad97  Replace the greenhouse latch"
        );
        let mut unchanged = response.clone();
        unchanged.as_object_mut().unwrap().remove("created");
        unchanged["changed"] = json!(false);
        let (message, _) =
            write_output(&serde_json::from_value(unchanged).unwrap(), "Done").unwrap();
        assert!(message.starts_with("Already done 97ad97a  Replace the greenhouse latch"));
        assert_eq!(message.lines().count(), 2);
        let mut changed = response.clone();
        changed.as_object_mut().unwrap().remove("created");
        changed["targets"] = json!([id]);
        changed["tasks"][0]["title"] = json!("Install the greenhouse latch");
        let changed: Response = serde_json::from_value(changed).unwrap();
        let (message, _) = write_output(&changed, "Done").unwrap();
        assert_eq!(message, "Done 97ad97  Install the greenhouse latch");
        response["targets"] = json!([id]);
        let earlier = serde_json::from_value(response.clone()).unwrap();
        let responses = [earlier, changed];
        let combined = merge_write_responses(&responses).unwrap();
        let write = combined.write().unwrap();
        assert_eq!(write.targets, [id]);
        assert_eq!(write.tasks.len(), 2);
        assert_eq!(write.tasks[0].title, "Install the greenhouse latch");
        let ids = ShortIds::new([id, peer]);
        let created: Response = serde_json::from_value(response.clone()).unwrap();
        let (message, _) = write_output_with_ids(&created, "Created", Some(&ids)).unwrap();
        assert_eq!(message, "Created 97ad97a  Replace the greenhouse latch");

        response["edges"] =
            json!([{"from":id,"to":peer,"kind":"blocks","effective":false,"reason":"cycle"}]);
        let (message, warnings) = render(response);
        assert_eq!(warnings[0].details["edge"]["reason"], "cycle");
        assert_eq!(message, "Created 97ad97a  Replace the greenhouse latch");
        assert_eq!(
            warnings[0].message,
            "recorded, but not in effect: 97ad97a blocks 97ad97b would create a cycle"
        );
    }
}

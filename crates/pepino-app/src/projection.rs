use pepino_core::TaskId;
use pepino_store::{
    BoardView, CommentRecord as StoredComment, EdgeExclusion, EdgeRecord, StoreError,
    TaskPrefixResolution, TaskRecord as StoredTask,
};

use crate::app::{invalid, store_error};
use crate::{
    AppError, CommentRecord, ConflictReference, EdgeEffect, EdgeExclusionReason, MissingReference,
    TaskRecord, TaskSummary,
};

pub(super) fn resolve<'a>(
    view: &'a BoardView,
    field: &str,
    reference: &str,
) -> Result<&'a StoredTask, AppError> {
    snapshot_task(view, resolve_id(view, field, reference, false)?)
}

pub(super) fn snapshot_task(view: &BoardView, id: TaskId) -> Result<&StoredTask, AppError> {
    view.task(id).ok_or_else(|| {
        store_error(StoreError::ViewValue(format!(
            "missing task {id} in board snapshot"
        )))
    })
}

pub(super) fn resolve_id(
    view: &BoardView,
    field: &str,
    reference: &str,
    include_deleted: bool,
) -> Result<TaskId, AppError> {
    match view.resolve_task_prefix(reference) {
        Ok(TaskPrefixResolution::Live(id)) => Ok(id),
        Ok(TaskPrefixResolution::Deleted { id: Some(id) }) if include_deleted => Ok(id),
        Ok(TaskPrefixResolution::Ambiguous { count, candidates }) => Err(AppError::AmbiguousId {
            field: field.into(),
            task: reference.into(),
            count,
            candidates: candidates.iter().map(ToString::to_string).collect(),
        }),
        Ok(TaskPrefixResolution::Unknown) => Err(AppError::NotFound {
            field: field.into(),
            reference: MissingReference::Task(reference.into()),
        }),
        Ok(TaskPrefixResolution::Deleted { id }) => Err(AppError::Conflict {
            field: field.into(),
            reference: ConflictReference::Task {
                task: id.map_or_else(|| reference.into(), |id| id.to_string()),
                title: None,
            },
        }),
        Err(StoreError::InvalidPrefix) => Err(invalid(
            field,
            format!("invalid task reference {reference:?}: expected 4 to 64 hexadecimal digits"),
        )),
        Err(error) => Err(store_error(error)),
    }
}

pub(super) fn project(
    task: &StoredTask,
    comments: &[StoredComment],
) -> Result<TaskRecord, AppError> {
    Ok(TaskRecord {
        summary: project_summary(task)?,
        comments: comments
            .iter()
            .map(|comment| CommentRecord {
                id: comment.id.to_string(),
                author: comment.author.to_string(),
                hlc: comment.hlc.into(),
                text: comment.text.clone(),
            })
            .collect(),
    })
}

pub(super) fn project_at(view: &BoardView, id: TaskId) -> Result<TaskSummary, AppError> {
    project_summary(snapshot_task(view, id)?)
}

pub(super) fn project_summary(task: &StoredTask) -> Result<TaskSummary, AppError> {
    Ok(TaskSummary {
        id: task.id.to_string(),
        title: task.title.clone(),
        description: task.description.clone(),
        status: task.status.try_into()?,
        due: task.due.map(Into::into),
        priority: task.priority,
        fields: task
            .custom
            .iter()
            .map(|(name, value)| Ok((name.as_str().into(), value.clone().try_into()?)))
            .collect::<Result<_, AppError>>()?,
        tags: task.tags.clone(),
        assignees: task.assignees.iter().map(ToString::to_string).collect(),
        created: task.created.into(),
        updated: task.updated.into(),
        author: task.author.to_string(),
        parent: task.parent.map(|id| id.to_string()),
        children: task.children.iter().map(ToString::to_string).collect(),
        blocks: task.blocks.iter().map(ToString::to_string).collect(),
        blocked_by: task.blocked_by.iter().map(ToString::to_string).collect(),
        relates: task.relates.iter().map(ToString::to_string).collect(),
        blocked: task.blocked,
        ready: task.ready,
        comment_count: task.comment_count,
    })
}

pub(super) fn edge_effect(
    from: TaskId,
    kind: pepino_core::EdgeKind,
    to: TaskId,
    edge: Option<EdgeRecord>,
) -> Result<EdgeEffect, AppError> {
    Ok(EdgeEffect {
        from: from.to_string(),
        kind: kind.try_into()?,
        to: to.to_string(),
        effective: edge.is_some_and(|edge| edge.is_effective()),
        reason: edge
            .and_then(|edge| edge.exclusion)
            .map(|reason| match reason {
                EdgeExclusion::MissingEndpoint => EdgeExclusionReason::MissingEndpoint,
                EdgeExclusion::NewerParent => EdgeExclusionReason::SupersededParent,
                EdgeExclusion::Cycle => EdgeExclusionReason::Cycle,
            }),
    })
}

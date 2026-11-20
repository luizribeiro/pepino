use pepino_core::TaskId;
use pepino_store::{BoardView, StoreError, TaskPrefixResolution, TaskRecord as StoredTask};

use crate::app::{invalid, store_error};
use crate::{App, AppError, CommentRecord, TaskRecord};

pub(super) fn resolve<'a>(
    view: &'a BoardView,
    field: &str,
    reference: &str,
) -> Result<&'a StoredTask, AppError> {
    match view.resolve_task_prefix(reference) {
        Ok(TaskPrefixResolution::Live(id)) => Ok(view.task(id).expect("ID resolved in this view")),
        Ok(TaskPrefixResolution::Ambiguous { count, candidates }) => {
            Err(ambiguous(reference, count, candidates))
        }
        Ok(TaskPrefixResolution::Unknown) => Err(AppError::NotFound {
            task: reference.into(),
        }),
        Ok(TaskPrefixResolution::Deleted { id }) => Err(AppError::Conflict {
            task: id.map_or_else(|| reference.into(), |id| id.to_string()),
        }),
        Err(StoreError::InvalidPrefix) => Err(invalid(
            field,
            "expected a task reference of 4 to 64 hexadecimal digits",
        )),
        Err(error) => Err(store_error(error)),
    }
}

impl App {
    pub(super) async fn record(&self, task: &StoredTask) -> Result<TaskRecord, AppError> {
        let comments = self
            .store
            .comments(self.local.board_id, task.id)
            .await
            .map_err(store_error)?;
        Ok(TaskRecord {
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
            created: task.created,
            updated: task.updated,
            author: task.author.to_string(),
            parent: task.parent.map(|id| id.to_string()),
            children: task.children.iter().map(ToString::to_string).collect(),
            blocks: task.blocks.iter().map(ToString::to_string).collect(),
            blocked_by: task.blocked_by.iter().map(ToString::to_string).collect(),
            relates: task.relates.iter().map(ToString::to_string).collect(),
            blocked: task.blocked,
            ready: task.ready,
            comments: comments
                .into_iter()
                .map(|comment| CommentRecord {
                    id: comment.id.to_string(),
                    author: comment.author.to_string(),
                    hlc: comment.hlc,
                    text: comment.text,
                })
                .collect(),
        })
    }
}

fn ambiguous(reference: &str, count: u64, ids: impl IntoIterator<Item = TaskId>) -> AppError {
    AppError::AmbiguousId {
        task: reference.into(),
        count,
        candidates: ids.into_iter().map(|id| id.to_string()).collect(),
    }
}

use std::collections::{BTreeMap, BTreeSet};

use pepino_store::{BoardView, Dependency};

use crate::app::{invalid, store_error};
use crate::batch::deletion_impact;
use crate::projection::{edge_effect, project, project_at, project_summary, resolve, resolve_id};
use crate::{App, AppError, DependencyRecord, ReadRequest, Response, TreeNode};

impl App {
    pub(super) async fn read(&mut self, request: ReadRequest) -> Result<Response, AppError> {
        match request {
            ReadRequest::ListTasks {
                filter,
                sort,
                limit,
                today,
            } => {
                self.list_tasks(&filter, sort.as_deref(), limit, today, false)
                    .await
            }
            ReadRequest::ListOpenTasks {
                filter,
                sort,
                limit,
                today,
            } => {
                self.list_tasks(&filter, sort.as_deref(), limit, today, true)
                    .await
            }
            ReadRequest::PreviewDelete { tasks } => self.preview_delete(tasks).await,
            ReadRequest::ListViews => self.list_views().await,
            ReadRequest::Log { task, limit } => self.log(task.as_deref(), limit).await,
            ReadRequest::Status => self.status().await,
            ReadRequest::GetTask { task } => self.get_task(&task).await,
            ReadRequest::Tree { task } => self.tree(task.as_deref()).await,
            ReadRequest::Deps { task } => self.deps(&task).await,
        }
    }

    pub(super) async fn board(&self) -> Result<BoardView, AppError> {
        self.store
            .board(self.local.board_id)
            .await
            .map_err(store_error)
    }

    async fn preview_delete(&self, references: Vec<String>) -> Result<Response, AppError> {
        if references.is_empty() {
            return Err(invalid("tasks", "expected at least one task"));
        }
        let view = self.board().await?;
        let mut ids = BTreeSet::new();
        let mut targets = Vec::new();
        let mut tasks = Vec::new();
        for (index, reference) in references.iter().enumerate() {
            let id = resolve_id(&view, &format!("tasks[{index}]"), reference, true)?;
            if ids.insert(id) {
                targets.push(id.to_string());
                if let Some(task) = view.task(id) {
                    tasks.push(project_summary(task)?);
                }
            }
        }
        ids.retain(|id| view.task(*id).is_some());
        let (top_level, unblocked) = deletion_impact(&view, &view.edges_after_removing(&ids), &ids);
        let summarize = |ids: Vec<_>| {
            ids.into_iter()
                .map(|id| project_at(&view, id))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Response::DeletePreview {
            targets,
            tasks,
            top_level: summarize(top_level)?,
            unblocked: summarize(unblocked)?,
        })
    }

    async fn get_task(&self, reference: &str) -> Result<Response, AppError> {
        let board = self
            .store
            .board_with_comments(self.local.board_id)
            .await
            .map_err(store_error)?;
        let task = resolve(&board, "task", reference)?;
        let incident = board.edges(task.id);
        let peers: BTreeSet<_> = incident
            .iter()
            .flat_map(|edge| [edge.from, edge.to])
            .filter(|id| *id != task.id)
            .collect();
        let mut relation_titles = BTreeMap::new();
        let mut missing = Vec::new();
        for peer in peers {
            match board.task(peer) {
                Some(record) => {
                    relation_titles.insert(peer.to_string(), record.title.clone());
                }
                None => missing.push(peer),
            }
        }
        if !missing.is_empty() {
            relation_titles.extend(
                self.store
                    .task_titles(self.local.board_id, &missing)
                    .await
                    .map_err(store_error)?
                    .into_iter()
                    .map(|(id, title)| (id.to_string(), title)),
            );
        }
        let assignee_names = task
            .assignees
            .contains(&self.local.identity_id)
            .then(|| {
                (
                    self.local.identity_id.to_string(),
                    self.local.display_name.clone(),
                )
            })
            .into_iter()
            .collect();
        let edges = incident
            .into_iter()
            .filter(|edge| edge.exclusion.is_some())
            .map(|edge| edge_effect(edge.from, edge.kind, edge.to, Some(edge)))
            .collect::<Result<_, _>>()?;
        let task = project(task, board.comments(task.id))?;
        let names = self.local.names_for(
            task.assignees
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(task.author.as_str()))
                .chain(task.comments.iter().map(|comment| comment.author.as_str())),
        );
        Ok(Response::Task {
            task: Box::new(task),
            names,
            edges,
            relation_titles,
            assignee_names,
        })
    }

    async fn tree(&self, reference: Option<&str>) -> Result<Response, AppError> {
        let board = self.board().await?;
        let roots = match reference {
            Some(reference) => vec![resolve(&board, "task", reference)?.id],
            None => board
                .iter()
                .filter(|task| task.parent.is_none())
                .map(|task| task.id)
                .collect(),
        };
        let nodes = roots
            .into_iter()
            .flat_map(|root| board.subtree(root))
            .map(|node| {
                Ok(TreeNode {
                    depth: node.depth,
                    task: project_at(&board, node.task)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let names = self.local.task_names(nodes.iter().map(|node| &node.task));
        Ok(Response::Tree { nodes, names })
    }

    async fn deps(&self, reference: &str) -> Result<Response, AppError> {
        let board = self.board().await?;
        let task = resolve(&board, "task", reference)?;
        let deps = board.dependencies(task.id);
        let project_deps = |entries: Vec<Dependency>| {
            entries
                .into_iter()
                .map(|entry| {
                    Ok(DependencyRecord {
                        distance: entry.distance,
                        task: project_at(&board, entry.task)?,
                    })
                })
                .collect::<Result<Vec<_>, AppError>>()
        };
        let task = Box::new(project_summary(task)?);
        let blockers = project_deps(deps.blocked_by)?;
        let dependents = project_deps(deps.blocks)?;
        let names = self.local.task_names(
            std::iter::once(task.as_ref())
                .chain(blockers.iter().chain(&dependents).map(|entry| &entry.task)),
        );
        Ok(Response::Deps {
            task,
            blockers,
            dependents,
            names,
        })
    }
}

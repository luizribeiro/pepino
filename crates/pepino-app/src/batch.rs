use std::collections::BTreeSet;

use pepino_core::{
    Clock, DeviceId, EdgeKind, Envelope, Event, EventId, InvalidEvent, NewEnvelopeError, TaskId,
};
use pepino_store::{AppendOutcome, BoardView, EdgeExclusion, IdSource, StoreError};

use crate::app::{invalid, store_error};
use crate::projection::{edge_effect as project_edge, project_summary};
use crate::state::storage;
use crate::{App, AppError, ConflictReference, EdgeEffect, Response};

type RequiredEdge = ((TaskId, EdgeKind, TaskId), String);

pub(super) struct Batch {
    pub(super) targets: BTreeSet<TaskId>,
    events: Vec<Envelope>,
    required: Vec<RequiredEdge>,
    clock: Clock,
    author: DeviceId,
    now: u64,
}

impl Batch {
    pub(super) fn event_count(&self) -> usize {
        self.events.len()
    }

    pub(super) fn stage(
        &mut self,
        event: Event,
        field: &str,
        changed: bool,
    ) -> Result<(), AppError> {
        if changed {
            self.push(event, field)?;
        } else {
            event
                .validate()
                .map_err(|error| invalid_event(field, error))?;
            self.targets.extend(event.referenced_tasks());
        }
        Ok(())
    }

    pub(super) fn push(&mut self, event: Event, field: &str) -> Result<EventId, AppError> {
        let hlc = self
            .clock
            .tick(self.now)
            .map_err(|error| storage("advance event clock", error, false))?;
        let envelope = Envelope::new(self.author, hlc, event).map_err(|error| match error {
            NewEnvelopeError::InvalidEvent(error) => invalid_event(field, error),
            NewEnvelopeError::Encode(error) => storage("encode event", error, false),
        })?;
        self.targets.extend(envelope.created_task());
        self.targets.extend(envelope.event().referenced_tasks());
        if let Event::EdgeAdded { from, kind, to } = *envelope.event() {
            self.required.push(((from, kind, to), field.into()));
        }
        let id = envelope.id();
        self.events.push(envelope);
        Ok(id)
    }
}

pub(super) fn invalid_event(field: &str, error: InvalidEvent) -> AppError {
    match error {
        InvalidEvent::FieldName { reason, .. } => invalid(field, reason),
        InvalidEvent::FieldType(_) => invalid(field, "incorrect value type for built-in field"),
        InvalidEvent::BuiltInCustomField(_) => invalid(field, "expected a custom field name"),
        InvalidEvent::Tag(tag) => invalid(
            field,
            format!("invalid tag {tag:?}: expected a lowercase tag of at most 64 bytes"),
        ),
        error => invalid(field, error),
    }
}

impl App {
    pub(super) async fn batch(&self, now: u64) -> Result<Batch, AppError> {
        let stored = self
            .store
            .latest_hlc(self.local.board_id)
            .await
            .map_err(store_error)?;
        let clock = self
            .clock
            .latest()
            .max(stored)
            .map_or_else(Clock::new, Clock::from_hlc);
        Ok(Batch {
            targets: BTreeSet::new(),
            events: Vec::new(),
            required: Vec::new(),
            clock,
            author: self.local.device_id,
            now,
        })
    }

    pub(super) async fn commit_batch(
        &mut self,
        before: &BoardView,
        batch: Batch,
    ) -> Result<Response, AppError> {
        if batch.events.is_empty() {
            return self.write_response(before, before, false, None, batch.targets, Vec::new());
        }
        let encoded = batch
            .events
            .iter()
            .map(Envelope::encode)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("encode event batch", error, false))?;
        let writes: Vec<_> = encoded
            .iter()
            .map(|bytes| (bytes.as_slice(), IdSource::Content))
            .collect();
        let required: Vec<_> = batch.required.iter().map(|(edge, _)| *edge).collect();
        let outcomes = self
            .store
            .append_batch_requiring_edges(self.local.board_id, &writes, &required)
            .await
            .map_err(|error| match error {
                StoreError::IneffectiveEdge { from, to, reason } => {
                    let field = batch
                        .required
                        .iter()
                        .find(|((src, _, dst), _)| (*src, *dst) == (from, to))
                        .map_or("to", |(_, field)| field.as_str());
                    let reason = match reason {
                        Some(EdgeExclusion::Cycle) => {
                            format!("link would create a cycle through {from} and {to}")
                        }
                        Some(EdgeExclusion::MissingEndpoint) => {
                            format!("link requires live endpoints {from} and {to}")
                        }
                        _ => format!("link from {from} to {to} was superseded"),
                    };
                    AppError::Conflict {
                        field: field.into(),
                        reference: ConflictReference::Relation { reason },
                    }
                }
                error => store_error(error),
            })?;
        for event in &batch.events {
            let hlc = event.hlc();
            self.clock.observe(hlc, batch.now.max(hlc.wall_ms));
        }
        ensure_local_outcomes(&outcomes)?;
        let applied: Vec<_> = batch
            .events
            .iter()
            .zip(&outcomes)
            .filter_map(|(event, outcome)| {
                matches!(outcome, AppendOutcome::Applied { .. }).then_some(event)
            })
            .collect();
        if applied.is_empty() {
            return self.write_response(before, before, false, None, batch.targets, Vec::new());
        }

        let after = self
            .store
            .board(self.local.board_id)
            .await
            .map_err(|error| committed_error(store_error(error)))?;
        let created = applied.iter().find_map(|event| event.created_task());
        let edges = applied
            .iter()
            .filter_map(|event| edge_effect(&after, event.event()).transpose())
            .collect::<Result<Vec<_>, _>>()
            .map_err(committed_error)?;
        self.write_response(before, &after, true, created, batch.targets, edges)
            .map_err(committed_error)
    }

    fn write_response(
        &self,
        before: &BoardView,
        view: &BoardView,
        changed: bool,
        created: Option<TaskId>,
        targets: BTreeSet<TaskId>,
        edges: Vec<EdgeEffect>,
    ) -> Result<Response, AppError> {
        let mut affected = targets.clone();
        if changed {
            affected.extend(
                view.iter()
                    .filter(|task| before.task(task.id) != Some(*task))
                    .map(|task| task.id),
            );
        }
        let targets = match created {
            Some(id) => vec![id.to_string()],
            None => targets.into_iter().map(|id| id.to_string()).collect(),
        };
        let mut tasks = Vec::new();
        let mut deleted = Vec::new();
        for id in &affected {
            if let Some(task) = view.task(*id) {
                tasks.push(project_summary(task)?);
            } else if let Some(task) = before.task(*id) {
                deleted.push(project_summary(task)?);
            }
        }
        let (top_level, unblocked) = deletion_impact(
            before,
            view.edge_records(),
            &affected
                .into_iter()
                .filter(|id| before.task(*id).is_some() && view.task(*id).is_none())
                .collect(),
        );
        let summarize = |ids: Vec<TaskId>| {
            ids.into_iter()
                .filter_map(|id| view.task(id))
                .map(project_summary)
                .collect::<Result<Vec<_>, _>>()
        };
        let (top_level, unblocked) = (summarize(top_level)?, summarize(unblocked)?);
        let names = self.local.task_names(tasks.iter().chain(&deleted));
        Ok(Response::Write {
            targets,
            names,
            deleted,
            changed,
            created: created.map(|id| id.to_string()),
            tasks,
            edges,
            activated: activated_edges(before, view)?,
            top_level,
            unblocked,
        })
    }
}

pub(super) fn deletion_impact(
    before: &BoardView,
    remaining_edges: &[pepino_store::EdgeRecord],
    deleted: &BTreeSet<TaskId>,
) -> (Vec<TaskId>, Vec<TaskId>) {
    if deleted.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut has_parent = BTreeSet::new();
    let mut blocked = BTreeSet::new();
    for edge in remaining_edges.iter().filter(|edge| edge.is_effective()) {
        match edge.kind {
            EdgeKind::Parent => {
                has_parent.insert(edge.from);
            }
            EdgeKind::Blocks
                if before
                    .task(edge.from)
                    .is_some_and(|task| task.status.is_open()) =>
            {
                blocked.insert(edge.to);
            }
            _ => {}
        }
    }
    let mut top_level = Vec::new();
    let mut unblocked = Vec::new();
    for task in before.iter().filter(|task| !deleted.contains(&task.id)) {
        if task.parent.is_some() && !has_parent.contains(&task.id) {
            top_level.push(task.id);
        }
        if task.blocked && !blocked.contains(&task.id) {
            unblocked.push(task.id);
        }
    }
    (top_level, unblocked)
}

fn activated_edges(before: &BoardView, after: &BoardView) -> Result<Vec<EdgeEffect>, AppError> {
    let key = |edge: &pepino_store::EdgeRecord| (edge.from, edge.kind as u8, edge.to);
    let dormant: std::collections::HashSet<_> = before
        .edge_records()
        .iter()
        .filter(|edge| !edge.is_effective())
        .map(key)
        .collect();
    after
        .edge_records()
        .iter()
        .filter(|edge| edge.is_effective() && dormant.contains(&key(edge)))
        .map(|edge| project_edge(edge.from, edge.kind, edge.to, Some(*edge)))
        .collect()
}

fn ensure_local_outcomes(outcomes: &[AppendOutcome]) -> Result<(), AppError> {
    if outcomes
        .iter()
        .any(|outcome| matches!(outcome, AppendOutcome::Skipped { .. }))
    {
        return Err(committed_error(
            "internal invariant violation: store skipped a locally generated event",
        ));
    }
    Ok(())
}

fn committed_error(error: impl std::fmt::Display) -> AppError {
    AppError::WriteCommitted {
        reason: error.to_string(),
    }
}

fn edge_effect(view: &BoardView, event: &Event) -> Result<Option<EdgeEffect>, AppError> {
    let (from, kind, to) = match *event {
        Event::EdgeAdded { from, kind, to } | Event::EdgeRemoved { from, kind, to } => {
            (from, kind, to)
        }
        _ => return Ok(None),
    };
    let edge = view
        .edges(from)
        .into_iter()
        .find(|edge| (edge.from, edge.kind, edge.to) == (from, kind, to));
    project_edge(from, kind, to, edge).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skipped_local_event_is_a_committed_invariant_violation() {
        let error = ensure_local_outcomes(&[AppendOutcome::Skipped {
            id: EventId::from_bytes([31; 32]),
            reason: pepino_store::SkipReason::UnknownVersion(2),
        }])
        .unwrap_err();
        assert!(
            matches!(&error, AppError::WriteCommitted { reason } if reason.contains("internal invariant violation"))
        );
        assert!(!error.retryable());
    }
}

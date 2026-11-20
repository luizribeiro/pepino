use std::collections::BTreeSet;

use pepino_core::{
    Clock, DeviceId, Envelope, Event, EventId, InvalidEvent, NewEnvelopeError, TaskId,
};
use pepino_store::{AppendOutcome, BoardView, IdSource};

use crate::app::{invalid, store_error};
use crate::projection::{edge_effect as project_edge, project_summary};
use crate::state::storage;
use crate::{App, AppError, EdgeEffect, Response};

pub(super) struct Batch {
    pub(super) affected: BTreeSet<TaskId>,
    events: Vec<Envelope>,
    clock: Clock,
    author: DeviceId,
    now: u64,
}

impl Batch {
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
            self.affected.extend(event.referenced_tasks());
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
        self.affected.extend(envelope.created_task());
        self.affected.extend(envelope.event().referenced_tasks());
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
        InvalidEvent::Tag(_) => invalid(field, "invalid tag name"),
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
            affected: BTreeSet::new(),
            events: Vec::new(),
            clock,
            author: self.local.device_id,
            now,
        })
    }

    pub(super) async fn commit_batch(
        &mut self,
        before: &BoardView,
        mut batch: Batch,
    ) -> Result<Response, AppError> {
        if batch.events.is_empty() {
            return self.write_response(before, false, None, batch.affected, Vec::new());
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
        let outcomes = self
            .store
            .append_batch(self.local.board_id, &writes)
            .await
            .map_err(store_error)?;
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
            return self.write_response(before, false, None, batch.affected, Vec::new());
        }

        let after = self
            .store
            .board(self.local.board_id)
            .await
            .map_err(|error| committed_error(store_error(error)))?;
        batch.affected.extend(
            after
                .iter()
                .filter(|task| before.task(task.id) != Some(*task))
                .map(|task| task.id),
        );
        let created = applied.iter().find_map(|event| event.created_task());
        let edges = applied
            .iter()
            .filter_map(|event| edge_effect(&after, event.event()).transpose())
            .collect::<Result<Vec<_>, _>>()
            .map_err(committed_error)?;
        self.write_response(&after, true, created, batch.affected, edges)
            .map_err(committed_error)
    }

    fn write_response(
        &self,
        view: &BoardView,
        changed: bool,
        created: Option<TaskId>,
        affected: BTreeSet<TaskId>,
        edges: Vec<EdgeEffect>,
    ) -> Result<Response, AppError> {
        let mut tasks = Vec::new();
        for id in &affected {
            if let Some(task) = view.task(*id) {
                tasks.push(project_summary(task)?);
            }
        }
        Ok(Response::Write {
            changed,
            created: created.map(|id| id.to_string()),
            tasks,
            edges,
        })
    }
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

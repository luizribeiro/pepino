use std::collections::BTreeSet;

use pepino_core::{
    Clock, DeviceId, Envelope, Event, EventId, InvalidEvent, NewEnvelopeError, TaskId,
};
use pepino_store::{AppendOutcome, BoardView, IdSource};

use crate::app::{invalid, store_error};
use crate::state::storage;
use crate::{App, AppError, Response};

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

fn invalid_event(field: &str, error: InvalidEvent) -> AppError {
    let input = match &error {
        InvalidEvent::EmptyTitle => "title",
        InvalidEvent::TextTooLong { kind: "title", .. } => "title",
        InvalidEvent::TextTooLong {
            kind: "description",
            ..
        } => "description",
        InvalidEvent::Tag(_) if field == "fields" => "tags",
        _ => field,
    };
    invalid(input, error)
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
            return self.write_response(before, false, batch.affected).await;
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
        for event in batch.events {
            let hlc = event.hlc();
            self.clock.observe(hlc, batch.now.max(hlc.wall_ms));
        }
        if outcomes
            .iter()
            .any(|outcome| matches!(outcome, AppendOutcome::Skipped { .. }))
        {
            return Err(storage(
                "append event batch",
                "store skipped a locally generated event",
                false,
            ));
        }
        let changed = outcomes
            .iter()
            .any(|outcome| matches!(outcome, AppendOutcome::Applied { .. }));
        if !changed {
            return self.write_response(before, false, batch.affected).await;
        }

        let after = self
            .store
            .board(self.local.board_id)
            .await
            .map_err(committed_error)?;
        batch.affected.extend(
            before
                .iter()
                .filter(|task| after.task(task.id).is_none())
                .map(|task| task.id),
        );
        batch.affected.extend(
            after
                .iter()
                .filter(|task| before.task(task.id) != Some(*task))
                .map(|task| task.id),
        );
        self.write_response(&after, true, batch.affected)
            .await
            .map_err(committed_error)
    }

    async fn write_response(
        &self,
        view: &BoardView,
        changed: bool,
        affected: BTreeSet<TaskId>,
    ) -> Result<Response, AppError> {
        let mut tasks = Vec::new();
        for id in &affected {
            if let Some(task) = view.task(*id) {
                tasks.push(self.record(task).await?);
            }
        }
        Ok(Response::Write {
            changed,
            task_ids: affected.into_iter().map(|id| id.to_string()).collect(),
            tasks,
        })
    }
}

fn committed_error(error: impl std::fmt::Display) -> AppError {
    storage(
        "write committed but could not read the result",
        error,
        false,
    )
}

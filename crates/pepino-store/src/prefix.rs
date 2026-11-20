use pepino_core::TaskId;

use crate::{BoardView, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrefixResolution {
    None,
    One(TaskId),
    Ambiguous { count: u64, candidates: Vec<TaskId> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskPrefixResolution {
    Live(TaskId),
    /// The ID is included if the prefix identifies exactly one deleted task.
    Deleted {
        id: Option<TaskId>,
    },
    Unknown,
    Ambiguous {
        count: u64,
        candidates: Vec<TaskId>,
    },
}

impl BoardView {
    /// Resolves 4 to 64 hexadecimal digits against live task IDs.
    ///
    /// Ambiguous matches include the total count and up to ten candidates in ID order.
    pub fn resolve_prefix(&self, hex_prefix: &str) -> Result<PrefixResolution, StoreError> {
        let (first, last) = prefix_bounds(hex_prefix)?;
        Ok(self.resolve_live_prefix(first, last))
    }

    /// Resolves a task ID prefix, considering deleted tasks only if no live task matches.
    pub fn resolve_task_prefix(
        &self,
        hex_prefix: &str,
    ) -> Result<TaskPrefixResolution, StoreError> {
        let (first, last) = prefix_bounds(hex_prefix)?;
        Ok(match self.resolve_live_prefix(first, last) {
            PrefixResolution::One(id) => TaskPrefixResolution::Live(id),
            PrefixResolution::Ambiguous { count, candidates } => {
                TaskPrefixResolution::Ambiguous { count, candidates }
            }
            PrefixResolution::None => {
                let mut deleted = self.deleted_ids.range(first..=last);
                match (deleted.next(), deleted.next()) {
                    (None, _) => TaskPrefixResolution::Unknown,
                    (Some(&id), None) => TaskPrefixResolution::Deleted { id: Some(id) },
                    (Some(_), Some(_)) => TaskPrefixResolution::Deleted { id: None },
                }
            }
        })
    }

    fn resolve_live_prefix(&self, first: TaskId, last: TaskId) -> PrefixResolution {
        let mut matches = self.ids_in_range(first, last);
        let candidates: Vec<_> = matches.by_ref().take(10).collect();
        match candidates.as_slice() {
            [] => PrefixResolution::None,
            [task] => PrefixResolution::One(*task),
            _ => PrefixResolution::Ambiguous {
                count: (candidates.len() + matches.count()) as u64,
                candidates,
            },
        }
    }
}

fn prefix_bounds(hex_prefix: &str) -> Result<(TaskId, TaskId), StoreError> {
    if !(4..=64).contains(&hex_prefix.len())
        || !hex_prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StoreError::InvalidPrefix);
    }
    let first: TaskId = format!("{hex_prefix:0<64}")
        .parse()
        .map_err(|_| StoreError::InvalidPrefix)?;
    let last: TaskId = format!("{hex_prefix:f<64}")
        .parse()
        .map_err(|_| StoreError::InvalidPrefix)?;
    Ok((first, last))
}

use pepino_core::TaskId;

use crate::{BoardView, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrefixResolution {
    None,
    One(TaskId),
    Ambiguous { count: u64, candidates: Vec<TaskId> },
}

impl BoardView {
    /// Resolves 4 to 64 hexadecimal digits against live task IDs.
    ///
    /// Ambiguous matches include the total count and up to ten candidates in ID order.
    pub fn resolve_prefix(&self, hex_prefix: &str) -> Result<PrefixResolution, StoreError> {
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
        let mut matches = self.ids_in_range(first, last);
        let candidates: Vec<_> = matches.by_ref().take(10).collect();
        Ok(match candidates.as_slice() {
            [] => PrefixResolution::None,
            [task] => PrefixResolution::One(*task),
            _ => PrefixResolution::Ambiguous {
                count: (candidates.len() + matches.count()) as u64,
                candidates,
            },
        })
    }
}

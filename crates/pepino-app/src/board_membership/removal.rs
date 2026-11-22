use pepino_core::BoardId;

use crate::{App, AppError, LocalState, MembershipAction};

impl LocalState {
    pub fn board_removed(&self, board: BoardId) -> bool {
        self.latest_membership(board, self.own_person_key())
            .is_some_and(|record| record.change.action == MembershipAction::Removed)
    }

    pub fn board_removal_confirmed(&self, board: BoardId) -> bool {
        self.latest_membership(board, self.own_person_key())
            .is_some_and(|record| {
                record.change.action == MembershipAction::Removed
                    && (record.applied || self.catalog.observed_board_removals.contains(&board))
            })
    }
}

impl App {
    pub async fn observe_board_removal(
        &self,
        board: BoardId,
        removed: bool,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if removed {
                local.catalog.observed_board_removals.insert(board);
            } else {
                local.catalog.observed_board_removals.remove(&board);
            }
        })
        .await
    }
}

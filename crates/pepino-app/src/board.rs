use pepino_core::BoardId;

use crate::{AppError, LocalBoard, LocalState, MissingReference};

impl LocalState {
    pub fn resolve_board(&self, reference: &str, field: &str) -> Result<&LocalBoard, AppError> {
        if let Ok(id) = reference.parse::<BoardId>()
            && let Some(board) = self.boards().iter().find(|board| board.id == id)
        {
            return Ok(board);
        }

        let is_prefix = (4..64).contains(&reference.len())
            && reference.bytes().all(|byte| byte.is_ascii_hexdigit());
        let prefix = reference.to_ascii_lowercase();
        let matches: Vec<_> = self
            .boards()
            .iter()
            .filter(|board| {
                board.name == reference || (is_prefix && board.id.to_string().starts_with(&prefix))
            })
            .collect();
        match matches.as_slice() {
            [board] => Ok(board),
            [] => Err(AppError::NotFound {
                field: field.into(),
                reference: MissingReference::Board(reference.into()),
            }),
            _ => Err(AppError::AmbiguousBoard {
                field: field.into(),
                board: reference.into(),
                candidates: matches.into_iter().cloned().collect(),
            }),
        }
    }
}

#[cfg(test)]
#[test]
fn resolves_exact_ids_prefixes_and_duplicate_labels() {
    let first = BoardId::from_bytes([50; 32]);
    let second = BoardId::from_bytes([65; 32]);
    let mut local = LocalState::new(
        crate::OpenOptions {
            board_name: Some("Workshop".into()),
            ..Default::default()
        }
        .try_into()
        .unwrap(),
        pepino_core::DeviceId::from_bytes([25; 32]),
        first,
    )
    .unwrap();
    local.boards.push(LocalBoard {
        id: second,
        name: "Workshop".into(),
    });
    assert_eq!(
        local.resolve_board(&first.to_string(), "board").unwrap().id,
        first
    );
    assert_eq!(local.resolve_board("4141", "board").unwrap().id, second);
    assert!(matches!(
        local.resolve_board("Workshop", "board"),
        Err(AppError::AmbiguousBoard { candidates, .. })
        if candidates.iter().map(|board| board.id).eq([first, second])
    ));
    assert!(matches!(
        local.resolve_board("Greenhouse", "PPN_BOARD"),
        Err(AppError::NotFound { field, .. }) if field == "PPN_BOARD"
    ));
}

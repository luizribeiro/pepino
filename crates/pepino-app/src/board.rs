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

impl crate::App {
    pub(super) async fn missing_task(
        &self,
        field: String,
        task: String,
        relation: Option<(String, String)>,
        creating: bool,
    ) -> Result<AppError, AppError> {
        use crate::app::{invalid, store_error};
        use pepino_store::TaskPrefixResolution::{Ambiguous, Live};

        let selected = self.selected_board().id;
        let mut alternate_boards = Vec::new();
        let mut other_boards = Vec::new();
        let other = relation
            .as_ref()
            .map(|(from, to)| if field == "from" { to } else { from });
        for board in self.local.boards() {
            if board.id == selected && other.is_none() {
                continue;
            }
            let view = self.store.board(board.id).await.map_err(store_error)?;
            if board.id != selected
                && matches!(
                    view.resolve_task_prefix(&task),
                    Ok(Live(_) | Ambiguous { .. })
                )
            {
                alternate_boards.push(board.clone());
            }
            if let Some(other) = other
                && matches!(
                    view.resolve_task_prefix(other),
                    Ok(Live(_) | pepino_store::TaskPrefixResolution::Deleted { id: Some(_) })
                )
            {
                other_boards.push(board.id);
            }
        }
        if alternate_boards.is_empty() {
            return Ok(AppError::NotFound {
                field,
                reference: MissingReference::Task(task),
            });
        }
        let different_boards = !other_boards.is_empty()
            && (other_boards.contains(&selected)
                || !alternate_boards
                    .iter()
                    .any(|board| other_boards.contains(&board.id)));
        if creating || different_boards {
            return Ok(invalid(field, "both tasks must be on the same board"));
        }
        Ok(AppError::NotFound {
            field,
            reference: MissingReference::TaskOnOtherBoards {
                task,
                alternate_boards,
            },
        })
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

#[cfg(test)]
#[tokio::test]
async fn a_missing_task_can_match_another_board() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let mut local = app.local_state().clone();
    let id = BoardId::from_bytes([81; 32]);
    local.boards.push(LocalBoard {
        id,
        name: "Workshop".into(),
    });
    local.replace(directory.path()).unwrap();
    let request =
        serde_json::from_value(serde_json::json!({"op":"add_task","title":"Sand the workbench"}))
            .unwrap();
    let response = app
        .handle_on(request, 20, Some(("Workshop", "board")), false)
        .await
        .unwrap();
    let crate::Response::Write {
        created: Some(task),
        ..
    } = response
    else {
        panic!("expected task");
    };
    let request = crate::Request::Read(crate::ReadRequest::GetTask {
        task: task[..8].into(),
    });
    assert!(
        matches!(app.handle(request, 21).await, Err(AppError::NotFound {
        reference: MissingReference::TaskOnOtherBoards { alternate_boards, .. }, ..
    }) if alternate_boards[0].id == id)
    );
}

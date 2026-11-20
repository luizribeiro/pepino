mod support {
    pub mod template;
}

use pepino_app::{
    App, ReadRequest, Request, Response, WireFormat, WireRequest, WireResult, WriteRequest,
};
use pepino_core::BoardId;
use serde_json::json;

#[tokio::test]
async fn board_requests_route_tasks_and_update_only_local_current() {
    let root = tempfile::tempdir().unwrap();
    support::template::prepare(root.path()).await;
    let mut app = App::open(root.path(), 0).await.unwrap();
    let original = app.local_state().board_id();
    let workshop = BoardId::from_bytes([81; 32]);
    app.register_created_board(workshop, "Workshop")
        .await
        .unwrap();
    let request =
        json!({"v":1,"board":"5151","request":{"op":"add_task","title":"Measure ash boards"}});
    for format in [WireFormat::Json, WireFormat::Cbor] {
        let bytes = format.encode(&request).unwrap();
        let wire: WireRequest = format.decode(&bytes).unwrap();
        assert!(matches!(
            wire.handle(&mut app, 20).await.result,
            WireResult::Ok(Response::Write { .. })
        ));
    }
    let Response::Boards { boards } = app
        .handle(Request::Read(ReadRequest::BoardList), 0)
        .await
        .unwrap()
    else {
        panic!("expected boards");
    };
    assert_eq!(boards.len(), 2);
    assert_eq!(boards[0].board.id, original);
    assert!(boards[0].current);
    assert_eq!(boards[1].open, 2);
    let Response::Board { board } = app
        .handle(
            Request::Read(ReadRequest::BoardShow {
                board: Some("Workshop".into()),
            }),
            0,
        )
        .await
        .unwrap()
    else {
        panic!("expected a board");
    };
    assert_eq!(board.board.id, workshop);
    assert!(!board.current);
    let change = Request::Write(WriteRequest::BoardUse {
        board: "5151".into(),
    });
    assert!(
        matches!(app.handle(change, 0).await.unwrap(), Response::Board { board } if board.current)
    );
    let rename = Request::Write(WriteRequest::BoardRename {
        board: "5151".into(),
        name: "Wood Shop".into(),
    });
    assert!(
        matches!(app.handle(rename, 0).await.unwrap(), Response::Board { board } if board.board.name == "Wood Shop")
    );
    let reopened = App::open(root.path(), 0).await.unwrap();
    assert_eq!(reopened.local_state().board_id(), workshop);
    assert_eq!(reopened.local_state().catalog_board(), original);
    assert!(App::validate_board_name(" \t").is_err());

    for request in [
        Request::Read(ReadRequest::BoardShow {
            board: Some("Orchard".into()),
        }),
        Request::Write(WriteRequest::BoardUse {
            board: "Orchard".into(),
        }),
        Request::Write(WriteRequest::BoardRename {
            board: "Orchard".into(),
            name: "Orchard Annex".into(),
        }),
    ] {
        assert_eq!(
            app.handle(request, 0).await.unwrap_err().kind().code(),
            "not_found"
        );
    }
}

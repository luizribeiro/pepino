use pepino_core::*;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum FutureEvent {
    TaskCreated {
        title: String,
        fields: InitialFields,
    },
    TaskFieldSet {
        task: TaskId,
        field: FieldName,
        value: FieldValue,
    },
    TagAdded {
        task: TaskId,
        tag: String,
    },
    TagRemoved {
        task: TaskId,
        tag: String,
    },
    AssigneeAdded {
        task: TaskId,
        who: IdentityId,
    },
    AssigneeRemoved {
        task: TaskId,
        who: IdentityId,
    },
    EdgeAdded {
        from: TaskId,
        kind: EdgeKind,
        to: TaskId,
    },
    EdgeRemoved {
        from: TaskId,
        kind: EdgeKind,
        to: TaskId,
    },
    CommentAdded {
        task: TaskId,
        text: String,
    },
    TaskDeleted {
        task: TaskId,
    },
    ReminderAdded {
        task: TaskId,
        message: String,
    },
    BoardArchived,
}

#[test]
fn event_superset_preserves_known_encodings() {
    #[derive(Serialize)]
    struct Content {
        author: DeviceId,
        hlc: Hlc,
        event: FutureEvent,
    }

    let author = DeviceId::from_bytes([61; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 3,
    };
    let task = TaskId(EventId::from_bytes([59; 32]));
    let to = TaskId(EventId::from_bytes([63; 32]));
    let who = IdentityId::from_bytes([14; 32]);
    let encode = |event| {
        let mut bytes = Vec::new();
        ciborium::into_writer(&(1, Content { author, hlc, event }), &mut bytes).unwrap();
        bytes
    };
    let field = FieldName::new("description").unwrap();
    let field_for_future = field.clone();
    for (future, known) in [
        (
            FutureEvent::TaskCreated {
                title: "Repair the garden gate".into(),
                fields: InitialFields::default(),
            },
            Event::TaskCreated {
                title: "Repair the garden gate".into(),
                fields: InitialFields::default(),
            },
        ),
        (
            FutureEvent::TaskFieldSet {
                task,
                field: field_for_future,
                value: FieldValue::Unset,
            },
            Event::TaskFieldSet {
                task,
                field,
                value: FieldValue::Unset,
            },
        ),
        (
            FutureEvent::TagAdded {
                task,
                tag: "garden".into(),
            },
            Event::TagAdded {
                task,
                tag: "garden".into(),
            },
        ),
        (
            FutureEvent::TagRemoved {
                task,
                tag: "garden".into(),
            },
            Event::TagRemoved {
                task,
                tag: "garden".into(),
            },
        ),
        (
            FutureEvent::AssigneeAdded { task, who },
            Event::AssigneeAdded { task, who },
        ),
        (
            FutureEvent::AssigneeRemoved { task, who },
            Event::AssigneeRemoved { task, who },
        ),
        (
            FutureEvent::EdgeAdded {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
            Event::EdgeAdded {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
        ),
        (
            FutureEvent::EdgeRemoved {
                from: task,
                kind: EdgeKind::Parent,
                to,
            },
            Event::EdgeRemoved {
                from: task,
                kind: EdgeKind::Parent,
                to,
            },
        ),
        (
            FutureEvent::CommentAdded {
                task,
                text: "Pick up hinges".into(),
            },
            Event::CommentAdded {
                task,
                text: "Pick up hinges".into(),
            },
        ),
        (
            FutureEvent::TaskDeleted { task },
            Event::TaskDeleted { task },
        ),
    ] {
        let tagged = serde_json::to_value(&future).unwrap();
        assert!(tagged.get(known.kind()).is_some());
        let target = if matches!(known, Event::TaskCreated { .. }) {
            None
        } else {
            Some(task)
        };
        assert_eq!(known.task(), target);
        let bytes = encode(future);
        let expected = Envelope::new(author, hlc, known).unwrap();
        assert_eq!(
            expected.created_task(),
            target.is_none().then_some(TaskId(expected.id()))
        );
        assert_eq!(
            Envelope::decode(&bytes).unwrap(),
            DecodeOutcome::Known(expected)
        );
    }
    for (future, variant) in [
        (
            FutureEvent::ReminderAdded {
                task,
                message: "Pick up hinges".into(),
            },
            "reminder_added",
        ),
        (FutureEvent::BoardArchived, "board_archived"),
    ] {
        let bytes = encode(future);
        let expected = |id| DecodeOutcome::UnknownEvent {
            variant: variant.into(),
            id,
            author,
            hlc,
            bytes: bytes.clone(),
        };
        let id = EventId::from_bytes([12; 32]);
        assert_eq!(
            Envelope::decode(&bytes).unwrap(),
            expected(EventId::from_bytes(*blake3::hash(&bytes).as_bytes()))
        );
        let decoded = Envelope::decode_with_id(id, &bytes).unwrap();
        assert_eq!(decoded.id(), id);
        assert_eq!(decoded.author(), Some(author));
        assert_eq!(decoded.hlc(), Some(hlc));
        assert_eq!(decoded, expected(id));
        assert!(Envelope::decode(&bytes[..bytes.len() - 1]).is_err());
    }
}

use ciborium::Value;
use pepino_core::{
    Date, DeviceId, Envelope, Event, EventId, FieldName, FieldValue, Hlc, IdentityId,
    InitialFields, Status, TaskId,
};
use proptest::prelude::*;

pub(super) struct History {
    pub tasks: Vec<TaskId>,
    pub events: Vec<LoggedEvent>,
}

pub(super) struct LoggedEvent {
    pub id: EventId,
    pub bytes: Vec<u8>,
    pub classification: &'static str,
}

pub(super) fn histories() -> impl Strategy<Value = (usize, Vec<Change>)> {
    (
        3..=5usize,
        prop::collection::vec((0..=5usize, 0..8usize, 0..15usize, 0..3usize), 12..48),
    )
}

type Change = (usize, usize, usize, usize);

impl History {
    pub(super) fn new((count, changes): (usize, Vec<Change>)) -> Self {
        let mut history = Self {
            tasks: Vec::new(),
            events: Vec::new(),
        };
        let site = FieldName::new("supplier").unwrap();
        for (index, title) in [
            "Repair the garden gate",
            "Book the boiler service",
            "Prime the porch rail",
            "Label the spare keys",
            "Move the seedlings into the greenhouse",
        ]
        .into_iter()
        .take(count)
        .enumerate()
        {
            let author = index;
            let id = history
                .add(
                    author,
                    0,
                    Event::TaskCreated {
                        title: title.into(),
                        fields: InitialFields {
                            description: Some("Check parts and tools in the shed".into()),
                            due: Date::from_ymd_opt(2026, 11, 24 + index as u32),
                            priority: Some(index as i64),
                            tags: ["home".into(), "weekend".into()].into(),
                            assignees: [identity(author)].into(),
                            custom: [(
                                site.clone(),
                                FieldValue::Text("Oak Street Hardware".into()),
                            )]
                            .into(),
                            ..InitialFields::default()
                        },
                    },
                )
                .created_task()
                .unwrap();
            history.tasks.push(id);
        }
        let last = history.tasks[count - 1];
        let missing = TaskId::from_bytes([0x91; 32]);
        history.tasks.push(missing);
        history.add(
            7,
            1,
            Event::TagAdded {
                task: missing,
                tag: "urgent".into(),
            },
        );
        history.add(
            0,
            1,
            Event::CommentAdded {
                task: last,
                text: "Tools are on the lower shelf".into(),
            },
        );
        for (index, (task, author, action, variant)) in changes.into_iter().enumerate() {
            let task = history.tasks[task % history.tasks.len()];
            let tag = ["home", "weekend", "needs-parts"][variant].into();
            let who = identity(variant);
            let event = match action {
                0 => set(
                    task,
                    "status",
                    FieldValue::Status([Status::Todo, Status::Doing, Status::Done][variant]),
                ),
                1 => set(task, "description", FieldValue::Unset),
                2 => set(
                    task,
                    "description",
                    FieldValue::Text("Pick up the materials after work".into()),
                ),
                3 => set(
                    task,
                    "due",
                    FieldValue::Date(Date::from_ymd_opt(2026, 12, 1 + variant as u32).unwrap()),
                ),
                4 => set(task, "priority", FieldValue::Integer(variant as i64)),
                5 => set(
                    task,
                    "supplier",
                    FieldValue::Text("West End Hardware".into()),
                ),
                6 => set(task, "supplier", FieldValue::Unset),
                7 => Event::TagAdded { task, tag },
                8 => Event::TagRemoved { task, tag },
                9 => Event::AssigneeAdded { task, who },
                10 => Event::AssigneeRemoved { task, who },
                11 if task != last => Event::TaskDeleted { task },
                12 => set(
                    task,
                    "title",
                    FieldValue::Text("Follow up on the service appointment".into()),
                ),
                _ => Event::CommentAdded {
                    task,
                    text: [
                        "Check the estimate before ordering",
                        "I can help on Saturday",
                        "The shop closes at five",
                    ][variant]
                        .into(),
                },
            };
            history.add(author, 2 + (index % 4) as u64, event);
        }
        history.add_skipped();
        history
    }

    fn add(&mut self, author: usize, tick: u64, event: Event) -> Envelope {
        let hlc = Hlc {
            wall_ms: (1 << 63) - 1 + tick / 2,
            counter: (tick % 2) as u32,
        };
        let envelope =
            Envelope::new(DeviceId::from_bytes([51 + author as u8; 32]), hlc, event).unwrap();
        self.events.push(LoggedEvent {
            id: envelope.id(),
            bytes: envelope.encode().unwrap(),
            classification: "known",
        });
        envelope
    }

    fn add_skipped(&mut self) {
        let template = Envelope::new(
            DeviceId::from_bytes([59; 32]),
            Hlc {
                wall_ms: u64::MAX,
                counter: 4,
            },
            Event::TaskCreated {
                title: "Check the greenhouse heater".into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap()
        .encode()
        .unwrap();

        for (index, classification) in ["unknown_version", "unknown_event", "invalid"]
            .into_iter()
            .enumerate()
        {
            let mut value: Value = ciborium::from_reader(template.as_slice()).unwrap();
            let frame = value.as_array_mut().unwrap();
            match classification {
                "unknown_version" => frame[0] = Value::from(2),
                "unknown_event" => {
                    find(&mut frame[1], "event").as_map_mut().unwrap()[0].0 =
                        Value::from("reminder_added")
                }
                _ => {
                    let event = find(&mut frame[1], "event");
                    *find(find(event, "task_created"), "title") = Value::from("");
                }
            }
            let mut bytes = Vec::new();
            ciborium::into_writer(&value, &mut bytes).unwrap();
            self.events.insert(
                2 * index + 1,
                LoggedEvent {
                    id: Envelope::decode(&bytes).unwrap().id(),
                    bytes,
                    classification,
                },
            );
        }
    }
}

fn find<'a>(map: &'a mut Value, key: &str) -> &'a mut Value {
    &mut map
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(name, _)| name.as_text() == Some(key))
        .unwrap()
        .1
}

fn identity(author: usize) -> IdentityId {
    IdentityId::from_bytes([31 + author as u8; 32])
}

fn set(task: TaskId, field: &str, value: FieldValue) -> Event {
    Event::TaskFieldSet {
        task,
        field: FieldName::new(field).unwrap(),
        value,
    }
}

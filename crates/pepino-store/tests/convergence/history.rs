use pepino_core::{
    Clock, Date, DeviceId, Envelope, Event, FieldName, FieldValue, IdentityId, InitialFields,
    Status, TaskId,
};
use proptest::prelude::*;

pub(super) struct History {
    pub tasks: Vec<TaskId>,
    pub events: Vec<Vec<u8>>,
}

pub(super) fn histories() -> impl Strategy<Value = (usize, Vec<Change>)> {
    (
        3..=5usize,
        prop::collection::vec((0..5usize, 0..3usize, 0..15usize, 0..3usize), 12..48),
    )
}

type Change = (usize, usize, usize, usize);

impl History {
    pub(super) fn new((count, changes): (usize, Vec<Change>)) -> Self {
        let mut clocks = [const { Clock::new() }; 3];
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
            let author = index % clocks.len();
            let id = history
                .add(
                    &mut clocks,
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
        history.add(
            &mut clocks,
            0,
            1,
            Event::CommentAdded {
                task: last,
                text: "Tools are on the lower shelf".into(),
            },
        );
        for (index, (task, author, action, variant)) in changes.into_iter().enumerate() {
            let task = history.tasks[task % count];
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
            history.add(&mut clocks, author, 2 + (index / 3) as u64, event);
        }
        history
    }

    fn add(
        &mut self,
        clocks: &mut [Clock; 3],
        author: usize,
        millis: u64,
        event: Event,
    ) -> Envelope {
        let hlc = clocks[author].tick(1_795_144_800_000 + millis).unwrap();
        let envelope =
            Envelope::new(DeviceId::from_bytes([51 + author as u8; 32]), hlc, event).unwrap();
        self.events.push(envelope.encode().unwrap());
        envelope
    }
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

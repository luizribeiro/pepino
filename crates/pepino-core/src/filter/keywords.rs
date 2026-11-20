use crate::Status;

use super::ErrorKind;

macro_rules! keywords {
    ($visibility:vis enum $name:ident { $($variant:ident => $word:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $visibility enum $name { $($variant),+ }

        impl $name {
            pub(super) const WORDS: &'static [(Self, &'static str)] = &[
                $((Self::$variant, $word)),+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word),+
                }
            }
        }
    };
}

keywords! {
    pub(super) enum Builtin {
        Tag => "tag",
        Status => "status",
        Assignee => "assignee",
        Board => "board",
        Parent => "parent",
        Blocks => "blocks",
        BlockedBy => "blocked-by",
        Due => "due",
        Priority => "priority",
        Created => "created",
        Updated => "updated",
        Is => "is",
        Has => "has",
    }
}

keywords! {
    pub enum State {
        Blocked => "blocked",
        Ready => "ready",
        Open => "open",
        Closed => "closed",
    }
}

impl Builtin {
    pub(super) fn find(key: &str) -> Option<Self> {
        Self::WORDS
            .iter()
            .find_map(|&(builtin, word)| (word == key).then_some(builtin))
    }

    pub(super) fn correction(key: &str) -> Option<(Self, bool)> {
        let lower = key.to_ascii_lowercase();
        let alias = match lower.as_str() {
            "tags" => Some(Self::Tag),
            "assignees" => Some(Self::Assignee),
            "statuses" => Some(Self::Status),
            _ => None,
        };
        alias
            .or_else(|| Self::find(&lower))
            .map(|builtin| (builtin, key != lower))
    }
}

pub(super) const STATUSES: &[(Status, &str)] = &[
    (Status::Todo, status_word(Status::Todo)),
    (Status::Doing, status_word(Status::Doing)),
    (Status::Done, status_word(Status::Done)),
    (Status::Cancelled, status_word(Status::Cancelled)),
];

pub(super) const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Todo => "todo",
        Status::Doing => "doing",
        Status::Done => "done",
        Status::Cancelled => "cancelled",
    }
}

pub(super) fn read<T: Clone>(
    value: String,
    words: &[(T, &str)],
    unknown: impl FnOnce(String) -> ErrorKind,
) -> Result<T, ErrorKind> {
    lookup(&value, words).unwrap_or_else(|| Err(unknown(value)))
}

pub(super) fn lookup<T: Clone>(value: &str, words: &[(T, &str)]) -> Option<Result<T, ErrorKind>> {
    words.iter().find_map(|(item, word)| {
        if value == *word {
            Some(Ok(item.clone()))
        } else if value.eq_ignore_ascii_case(word) {
            Some(Err(ErrorKind::LowercaseKeyword {
                value: value.into(),
                lowercase: (*word).into(),
            }))
        } else {
            None
        }
    })
}

pub(super) fn accepted<T>(words: &[(T, &str)]) -> String {
    let Some(((_, last), rest)) = words.split_last() else {
        return String::new();
    };
    let head = rest
        .iter()
        .map(|(_, word)| *word)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{head}, or {last}")
}

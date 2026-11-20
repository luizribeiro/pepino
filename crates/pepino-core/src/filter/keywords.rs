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

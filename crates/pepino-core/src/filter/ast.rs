use crate::{FieldName, Status};

use super::keywords::{Builtin, State};
pub use chrono::NaiveDate as Date;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter(pub(super) Vec<Term>);

impl Filter {
    pub fn terms(&self) -> &[Term] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Term {
    pub negated: bool,
    pub clause: Clause,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Clause {
    Text(String),
    View(String),
    Is(Vec<State>),
    Has(Vec<PresenceField>),
    Tag(Vec<String>),
    Status(Vec<Status>),
    Assignee(Vec<Assignee>),
    Board(Vec<String>),
    Parent(Vec<String>),
    Blocks(Vec<String>),
    BlockedBy(Vec<String>),
    Match {
        key: String,
        alternatives: Vec<Comparison>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Assignee {
    Me,
    Person(String),
}

pub(super) const ME: &str = "me";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresenceField {
    Due,
    Parent,
    Assignee,
    Priority,
    Tag,
    Description,
    Custom(FieldName),
}

impl PresenceField {
    pub(super) const WORDS: &'static [(Self, &'static str)] = &[
        (Self::Due, Builtin::Due.as_str()),
        (Self::Parent, Builtin::Parent.as_str()),
        (Self::Assignee, Builtin::Assignee.as_str()),
        (Self::Priority, Builtin::Priority.as_str()),
        (Self::Tag, Builtin::Tag.as_str()),
        (Self::Description, Self::DESCRIPTION),
    ];

    const DESCRIPTION: &str = "description";

    pub fn as_str(&self) -> &str {
        match self {
            Self::Due => Builtin::Due.as_str(),
            Self::Parent => Builtin::Parent.as_str(),
            Self::Assignee => Builtin::Assignee.as_str(),
            Self::Priority => Builtin::Priority.as_str(),
            Self::Tag => Builtin::Tag.as_str(),
            Self::Description => Self::DESCRIPTION,
            Self::Custom(field) => field.as_str(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub operator: Operator,
    pub value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Text(String),
    Date(Date),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operator {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Operator {
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

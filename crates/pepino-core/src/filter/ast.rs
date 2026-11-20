use crate::{FieldName, Status};

use super::{
    DateOperand,
    keywords::{Builtin, State},
};
pub use crate::Date;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter(Vec<Term>);

impl Filter {
    /// Build a filter from syntax-valid terms in conjunction order.
    ///
    /// For canonical round trips, use nonempty alternatives and operands,
    /// valid tag and view names, and field names accepted for custom clauses.
    pub fn from_terms(terms: Vec<Term>) -> Self {
        Self(terms)
    }

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
    Priority(Vec<Comparison<i64>>),
    Due(Vec<Comparison<DateOperand>>),
    Created(Vec<Comparison<DateOperand>>),
    Updated(Vec<Comparison<DateOperand>>),
    Custom {
        field: FieldName,
        alternatives: Vec<Comparison<CustomValue>>,
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
pub struct Comparison<T> {
    pub operator: Operator,
    pub value: T,
}

/// The evaluator compares integers numerically, dates chronologically, and text
/// lexically. A quoted custom operand is always text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CustomValue {
    Integer(i64),
    Date(DateOperand),
    Text(String),
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

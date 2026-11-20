use crate::Status;

use super::ErrorKind;
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
    Has(Vec<String>),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Blocked,
    Ready,
    Open,
    Closed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub(super) fn parse(value: String) -> Result<Self, ErrorKind> {
        [Self::Blocked, Self::Ready, Self::Open, Self::Closed]
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or(ErrorKind::UnknownState { value })
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

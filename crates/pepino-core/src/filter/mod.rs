//! Parse task listing filters.

mod ast;
mod date;
mod display;
mod error;
mod keywords;
mod lex;
mod parse;

#[cfg(test)]
mod roundtrip;
#[cfg(test)]
mod tests;

pub use ast::{
    Assignee, Clause, Comparison, CustomValue, Date, Filter, Operator, PresenceField, Term,
};
pub use date::{DateOperand, RelativeDate};
pub use error::{ErrorKind, ParseError, Position};
pub use keywords::State;
pub use parse::{parse, parse_argv};

/// Names recognized by the parser for each built-in filter keyword group.
pub fn keyword_groups() -> [(&'static str, Vec<&'static str>); 4] {
    fn words<T>(entries: &[(T, &'static str)]) -> Vec<&'static str> {
        entries.iter().map(|(_, word)| *word).collect()
    }
    [
        ("Keys", words(keywords::Builtin::WORDS)),
        ("status:", words(keywords::STATUSES)),
        ("is:", words(State::WORDS)),
        ("has:", words(PresenceField::WORDS)),
    ]
}

use std::fmt::{self, Write as _};

use super::ast::ME;
use super::keywords::{Builtin, status_word};
use super::{Assignee, Clause, Filter, Term, Value};

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        joined(f, self.terms(), " ", |f, term| write!(f, "{term}"))
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negated {
            f.write_char('-')?;
        }
        match &self.clause {
            Clause::Text(text) => atom(f, text),
            Clause::View(name) => {
                f.write_char('@')?;
                atom(f, name)
            }
            Clause::Is(states) => {
                f.write_str("is:")?;
                joined(f, states, ",", |f, state| f.write_str(state.as_str()))
            }
            Clause::Has(fields) => {
                f.write_str("has:")?;
                joined(f, fields, ",", |f, field| atom(f, field))
            }
            Clause::Tag(tags) => texts(f, Builtin::Tag, tags),
            Clause::Status(statuses) => keyed(f, Builtin::Status, statuses, |f, status| {
                f.write_str(status_word(*status))
            }),
            Clause::Assignee(assignees) => {
                keyed(
                    f,
                    Builtin::Assignee,
                    assignees,
                    |f, assignee| match assignee {
                        Assignee::Me => f.write_str(ME),
                        Assignee::Person(name) if name == ME => quoted(f, name),
                        Assignee::Person(name) => atom(f, name),
                    },
                )
            }
            Clause::Board(boards) => texts(f, Builtin::Board, boards),
            Clause::Parent(parents) => texts(f, Builtin::Parent, parents),
            Clause::Blocks(tasks) => texts(f, Builtin::Blocks, tasks),
            Clause::BlockedBy(tasks) => texts(f, Builtin::BlockedBy, tasks),
            Clause::Match { key, alternatives } => {
                atom(f, key)?;
                f.write_char(':')?;
                joined(f, alternatives, ",", |f, comparison| {
                    f.write_str(comparison.operator.symbol())?;
                    match &comparison.value {
                        Value::Text(value) => atom(f, value),
                        Value::Date(date) => write!(f, "{date}"),
                    }
                })
            }
        }
    }
}

fn texts(f: &mut fmt::Formatter<'_>, key: Builtin, values: &[String]) -> fmt::Result {
    keyed(f, key, values, |f, value| atom(f, value))
}

fn keyed<T>(
    f: &mut fmt::Formatter<'_>,
    key: Builtin,
    values: &[T],
    write_item: impl Fn(&mut fmt::Formatter<'_>, &T) -> fmt::Result,
) -> fmt::Result {
    f.write_str(key.as_str())?;
    f.write_char(':')?;
    joined(f, values, ",", write_item)
}

fn joined<T>(
    f: &mut fmt::Formatter<'_>,
    items: &[T],
    separator: &str,
    write_item: impl Fn(&mut fmt::Formatter<'_>, &T) -> fmt::Result,
) -> fmt::Result {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            f.write_str(separator)?;
        }
        write_item(f, item)?;
    }
    Ok(())
}

fn atom(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    if !value.starts_with(['-', '@', '<', '>', '='])
        && !value.starts_with("//")
        && !value.contains([':', ',', '"'])
        && !value.contains(char::is_whitespace)
    {
        return f.write_str(value);
    }

    quoted(f, value)
}

fn quoted(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_char('"')?;
    for ch in value.chars() {
        if ch.is_whitespace() {
            write!(f, r"\u{{{:x}}}", u32::from(ch))?;
            continue;
        }
        if matches!(ch, '"' | '\\') {
            f.write_char('\\')?;
        }
        f.write_char(ch)?;
    }
    f.write_char('"')
}

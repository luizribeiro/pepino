use std::fmt::{self, Write as _};

use super::{Clause, Filter, Term, Value};

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

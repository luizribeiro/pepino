use crate::{FieldName, field::valid_name};

use super::keywords::Builtin;

use super::{
    Clause, Comparison, Date, ErrorKind, Filter, Operator, ParseError, Position, State, Term,
    Value, date, lex,
};

/// Parse whitespace-separated terms using the supplied calendar date.
///
/// The displayed filter stores resolved dates in calendar form.
///
/// ```
/// use pepino_core::filter::{Date, parse, parse_argv};
///
/// let today = Date::from_ymd_opt(2026, 11, 26).unwrap();
/// let filter = parse(r#"tag:garden due:<fri "fix the fence""#, today)?;
/// let argv = ["tag:garden", "due:<fri", "fix the fence"];
/// assert_eq!(parse_argv(&argv, today)?, filter);
/// assert_eq!(
///     filter.to_string(),
///     r#"tag:garden due:<2026-11-27 "fix\u{20}the\u{20}fence""#,
/// );
/// # Ok::<(), pepino_core::filter::ParseError>(())
/// ```
pub fn parse(input: &str, today: Date) -> Result<Filter, ParseError> {
    let mut terms = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        let position = Position::Byte(input.len() - rest.len());
        let end = lex::term_end(rest)
            .map_err(|offset| error(rest, position, (offset, ErrorKind::UnterminatedQuote)))?;
        terms.push(parse_term(&rest[..end], position, today)?);
        rest = rest[end..].trim_start();
    }
    Ok(Filter(terms))
}

/// Parse arguments that the caller has already split into individual terms.
///
/// A keyed argument keeps whitespace within its operands. Other arguments with
/// internal whitespace are literal text. Keep filter-level quotes around
/// keyed values to protect comma punctuation, as in `board:"Tools, Seeds"`.
pub fn parse_argv<S: AsRef<str>>(argv: &[S], today: Date) -> Result<Filter, ParseError> {
    argv.iter()
        .enumerate()
        .map(|(index, raw)| parse_argument(raw.as_ref(), index, today))
        .collect::<Result<Vec<_>, _>>()
        .map(Filter)
}

fn parse_argument(input: &str, index: usize, today: Date) -> Result<Term, ParseError> {
    let start = input.trim_start();
    let raw = start.trim_end();
    let position = Position::Argument {
        index,
        byte: input.len() - start.len(),
    };
    if raw.is_empty() {
        return Err(error(input, position, (0, ErrorKind::EmptyTerm)));
    }
    if raw.contains(char::is_whitespace) {
        let body = raw.strip_prefix('-').unwrap_or(raw);
        let keyed = key_parts(body)
            .is_some_and(|(_, values)| values.starts_with(|ch: char| !ch.is_whitespace()));
        let quoted_view = body.starts_with("@\"")
            && lex::characters(&body[1..])
                .any(|item| matches!(item, Ok((_, ch, true)) if ch.is_whitespace()));
        if !keyed && !quoted_view {
            return Ok(Term {
                negated: false,
                clause: Clause::Text(raw.into()),
            });
        }
    }
    parse_term(raw, position, today)
}

fn parse_term(raw: &str, position: Position, today: Date) -> Result<Term, ParseError> {
    let body = raw.strip_prefix('-');
    let clause = parse_clause(body.unwrap_or(raw), today)
        .map_err(|failure| error(raw, position, shifted(failure, usize::from(body.is_some()))))?;
    Ok(Term {
        negated: body.is_some(),
        clause,
    })
}

fn parse_clause(body: &str, today: Date) -> Located<Clause> {
    if let Some(name) = body.strip_prefix('@') {
        return parse_view(name).map_err(|failure| shifted(failure, 1));
    }
    if let Some((key, values)) = key_parts(body) {
        let key = key.map_err(|kind| (0, kind))?;
        return parse_keyed(key.as_str().into(), values, today)
            .map_err(|failure| shifted(failure, body.len() - values.len()));
    }
    decode_required(body, ErrorKind::EmptyTerm).map(Clause::Text)
}

fn key_parts(body: &str) -> Option<(Result<FieldName, ErrorKind>, &str)> {
    let (key, values) = body.split_once(':')?;
    if values.starts_with("//") {
        return None;
    }
    if Builtin::find(key).is_none()
        && let Some((suggested, wrong_case)) = Builtin::correction(key)
    {
        let kind = if wrong_case {
            ErrorKind::LowercaseKeyword {
                value: key.into(),
                lowercase: suggested.as_str().into(),
            }
        } else {
            ErrorKind::UnknownKey {
                value: key.into(),
                suggested: suggested.as_str(),
            }
        };
        return Some((Err(kind), values));
    }
    Some((Ok(FieldName::new(key).ok()?), values))
}

fn parse_view(raw: &str) -> Located<Clause> {
    let name = decode_required(raw, ErrorKind::EmptyView)?;
    if name.len() <= 64 && valid_name(&name, b"") {
        Ok(Clause::View(name))
    } else {
        Err((0, ErrorKind::InvalidView { name }))
    }
}

fn parse_keyed(key: String, raw: &str, today: Date) -> Located<Clause> {
    let field = FieldKind::of(&key);
    match Builtin::find(&key) {
        Some(Builtin::Is) => {
            alternatives(raw, &key, false, |_, value| State::parse(value)).map(Clause::Is)
        }
        Some(Builtin::Has) => {
            alternatives(raw, &key, false, |_, value| parse_presence(value)).map(Clause::Has)
        }
        _ => {
            let alternatives =
                alternatives(raw, &key, field.supports_comparison(), |operator, value| {
                    let value = match field {
                        FieldKind::Date => Value::Date(date::resolve(&value, today)?),
                        FieldKind::Status
                            if !matches!(
                                value.as_str(),
                                "todo" | "doing" | "done" | "cancelled"
                            ) =>
                        {
                            return Err(ErrorKind::UnknownStatus { value });
                        }
                        _ => Value::Text(value),
                    };
                    Ok(Comparison { operator, value })
                })?;
            Ok(Clause::Match { key, alternatives })
        }
    }
}

fn parse_presence(field: String) -> Result<String, ErrorKind> {
    if matches!(field.as_str(), "due" | "parent" | "assignee")
        || FieldKind::of(&field) == FieldKind::Custom
    {
        return Ok(field);
    }
    Err(ErrorKind::UnsupportedPresence { field })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldKind {
    Exact,
    Status,
    Date,
    Priority,
    Predicate,
    Custom,
}

impl FieldKind {
    fn of(key: &str) -> Self {
        use Builtin::*;
        match Builtin::find(key) {
            Some(Tag | Assignee | Board | Parent | Blocks | BlockedBy) => Self::Exact,
            Some(Status) => Self::Status,
            Some(Due | Created | Updated) => Self::Date,
            Some(Priority) => Self::Priority,
            Some(Is | Has) => Self::Predicate,
            None => Self::Custom,
        }
    }

    fn supports_comparison(self) -> bool {
        matches!(self, Self::Date | Self::Priority | Self::Custom)
    }
}

fn alternatives<T>(
    raw: &str,
    key: &str,
    ordered: bool,
    mut type_operand: impl FnMut(Operator, String) -> Result<T, ErrorKind>,
) -> Located<Vec<T>> {
    lex::split(raw, ',')
        .map(|(offset, raw)| {
            let (operator, raw) = split_operator(raw);
            let operand_start = offset + operator.symbol().len();
            let value = decode_required(raw, ErrorKind::EmptyValue)
                .map_err(|failure| shifted(failure, operand_start))?;
            if operator != Operator::Eq && !ordered {
                return Err((offset, ErrorKind::UnsupportedComparison { key: key.into() }));
            }
            type_operand(operator, value).map_err(|kind| (operand_start, kind))
        })
        .collect()
}

fn split_operator(raw: &str) -> (Operator, &str) {
    [Operator::Le, Operator::Ge, Operator::Lt, Operator::Gt]
        .into_iter()
        .find_map(|op| raw.strip_prefix(op.symbol()).map(|value| (op, value)))
        .unwrap_or((Operator::Eq, raw))
}

type Located<T> = Result<T, (usize, ErrorKind)>;

fn decode_required(raw: &str, empty: ErrorKind) -> Located<String> {
    let value = lex::decode(raw).map_err(|offset| (offset, ErrorKind::UnterminatedQuote))?;
    if value.is_empty() {
        return Err((0, empty));
    }
    Ok(value)
}

fn shifted((offset, kind): (usize, ErrorKind), start: usize) -> (usize, ErrorKind) {
    (start + offset, kind)
}

fn error(term: &str, position: Position, (offset, kind): (usize, ErrorKind)) -> ParseError {
    let position = match position {
        Position::Byte(start) => Position::Byte(start + offset),
        Position::Argument { index, byte } => Position::Argument {
            index,
            byte: byte + offset,
        },
    };
    ParseError {
        term: term.to_owned(),
        position,
        kind,
    }
}

use crate::{FieldName, field::valid_name};

use super::ast::ME;
use super::keywords::{self, Builtin};

use super::{
    Assignee, Clause, Comparison, CustomValue, Date, ErrorKind, Filter, Operator, ParseError,
    Position, PresenceField, State, Term, date, lex,
};

/// Parse whitespace-separated terms using the supplied calendar date.
///
/// The displayed filter uses real whitespace inside quotes and stores
/// resolved dates in calendar form.
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
///     r#"tag:garden due:<2026-11-27 "fix the fence""#,
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
    Ok(Filter::from_terms(terms))
}

/// Parse arguments that the caller has already split into individual terms.
///
/// A keyed argument keeps whitespace within its operands. Other arguments with
/// internal whitespace are literal text. An explicitly quoted saved view name
/// containing whitespace is an error. Keep filter-level quotes around keyed
/// values to protect comma punctuation, as in `board:"Tools, Seeds"`.
pub fn parse_argv<S: AsRef<str>>(argv: &[S], today: Date) -> Result<Filter, ParseError> {
    argv.iter()
        .enumerate()
        .map(|(index, raw)| parse_argument(raw.as_ref(), index, today))
        .collect::<Result<Vec<_>, _>>()
        .map(Filter::from_terms)
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
        return parse_keyed(key, values, today)
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

fn parse_keyed(field: FieldName, raw: &str, today: Date) -> Located<Clause> {
    use Builtin::*;
    let key = field.as_str();
    let equal_text = || alternatives(raw, key, false, |operand| Ok(operand.value));
    let dates = || {
        alternatives(raw, key, true, |operand| {
            let value = date::resolve(&operand.value, today)?;
            Ok(Comparison {
                operator: operand.operator,
                value,
            })
        })
    };
    match Builtin::find(key) {
        Some(Tag) => {
            alternatives(raw, key, false, |operand| parse_tag(operand.value)).map(Clause::Tag)
        }
        Some(Status) => alternatives(raw, key, false, |operand| {
            keywords::read(operand.value, keywords::STATUSES, |value| {
                ErrorKind::UnknownStatus { value }
            })
        })
        .map(Clause::Status),
        Some(Assignee) => alternatives(raw, key, false, |operand| Ok(parse_assignee(operand)))
            .map(Clause::Assignee),
        Some(Board) => equal_text().map(Clause::Board),
        Some(Parent) => equal_text().map(Clause::Parent),
        Some(Blocks) => equal_text().map(Clause::Blocks),
        Some(BlockedBy) => equal_text().map(Clause::BlockedBy),
        Some(Is) => alternatives(raw, key, false, |operand| {
            keywords::read(operand.value, State::WORDS, |value| {
                ErrorKind::UnknownState { value }
            })
        })
        .map(Clause::Is),
        Some(Has) => {
            alternatives(raw, key, false, |operand| parse_presence(operand.value)).map(Clause::Has)
        }
        Some(Priority) => alternatives(raw, key, true, |operand| {
            let value = parse_integer(operand.value)?;
            Ok(Comparison {
                operator: operand.operator,
                value,
            })
        })
        .map(Clause::Priority),
        Some(Due) => dates().map(Clause::Due),
        Some(Created) => dates().map(Clause::Created),
        Some(Updated) => dates().map(Clause::Updated),
        None => {
            let alternatives =
                alternatives(raw, key, true, |operand| parse_custom(operand, today))?;
            Ok(Clause::Custom {
                field,
                alternatives,
            })
        }
    }
}

fn parse_custom(operand: Operand, today: Date) -> Result<Comparison<CustomValue>, ErrorKind> {
    let Operand {
        operator,
        value,
        quoted,
    } = operand;
    let digits = value.strip_prefix(['+', '-']).unwrap_or(&value);
    let value = if quoted {
        CustomValue::Text(value)
    } else if lex::ascii_digits(digits) {
        CustomValue::Integer(parse_integer(value)?)
    } else {
        match date::resolve(&value, today) {
            Ok(date) => CustomValue::Date(date),
            Err(error @ ErrorKind::DateOutOfRange { .. }) => return Err(error),
            Err(_) => CustomValue::Text(value),
        }
    };
    Ok(Comparison { operator, value })
}

fn parse_integer(value: String) -> Result<i64, ErrorKind> {
    value
        .parse()
        .map_err(|_| ErrorKind::InvalidInteger { value })
}

fn parse_tag(value: String) -> Result<String, ErrorKind> {
    if valid_name(&value, b"./") {
        Ok(value)
    } else {
        Err(ErrorKind::InvalidTag { value })
    }
}

fn parse_assignee(operand: Operand) -> Assignee {
    if !operand.quoted && operand.value == ME {
        Assignee::Me
    } else {
        Assignee::Person(operand.value)
    }
}

fn parse_presence(field: String) -> Result<PresenceField, ErrorKind> {
    if let Some(known) = keywords::lookup(&field, PresenceField::WORDS) {
        return known;
    }
    if Builtin::find(&field).is_none()
        && let Ok(name) = FieldName::new(field.as_str())
        && name.is_custom()
        && Builtin::correction(&field).is_none()
    {
        return Ok(PresenceField::Custom(name));
    }
    Err(ErrorKind::UnsupportedPresence { field })
}

struct Operand {
    operator: Operator,
    value: String,
    quoted: bool,
}

fn alternatives<T>(
    raw: &str,
    key: &str,
    ordered: bool,
    mut type_operand: impl FnMut(Operand) -> Result<T, ErrorKind>,
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
            type_operand(Operand {
                operator,
                value,
                quoted: raw.contains('"'),
            })
            .map_err(|kind| (operand_start, kind))
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
        term: term.into(),
        position,
        reason: kind.reason().into_boxed_str(),
        hint: kind.hint().map(String::into_boxed_str),
        kind,
    }
}

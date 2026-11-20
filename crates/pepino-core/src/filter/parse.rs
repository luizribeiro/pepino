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
///     r#"tag:garden due:<2026-11-27 "fix the fence""#,
/// );
/// # Ok::<(), pepino_core::filter::ParseError>(())
/// ```
pub fn parse(input: &str, today: Date) -> Result<Filter, ParseError> {
    let mut terms = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        let position = Position::Byte(input.len() - rest.len());
        let end = lex::term_end(rest).map_err(|kind| error(rest, position, kind))?;
        terms.push(parse_term(&rest[..end], position, today)?);
        rest = rest[end..].trim_start();
    }
    Ok(Filter(terms))
}

/// Parse arguments that the caller has already split into individual terms.
///
/// Whitespace within an argument is literal. Keep filter-level quotes in an
/// argument to protect punctuation from its filter syntax, as in `tag:"tools,seeds"`.
pub fn parse_argv<S: AsRef<str>>(argv: &[S], today: Date) -> Result<Filter, ParseError> {
    argv.iter()
        .enumerate()
        .map(|(index, raw)| parse_term(raw.as_ref(), Position::Argument(index), today))
        .collect::<Result<Vec<_>, _>>()
        .map(Filter)
}

fn parse_term(raw: &str, position: Position, today: Date) -> Result<Term, ParseError> {
    let body = raw.strip_prefix('-');
    let clause =
        parse_clause(body.unwrap_or(raw), today).map_err(|kind| error(raw, position, kind))?;
    Ok(Term {
        negated: body.is_some(),
        clause,
    })
}

fn parse_clause(body: &str, today: Date) -> Result<Clause, ErrorKind> {
    if let Some(name) = body.strip_prefix('@') {
        return decode_required(name, ErrorKind::EmptyView).map(Clause::View);
    }
    if let Some((key, values)) = lex::split_once(body, ':') {
        let key = decode_required(key, ErrorKind::EmptyKey)?;
        return parse_keyed(key, values, today);
    }
    decode_required(body, ErrorKind::EmptyTerm).map(Clause::Text)
}

fn parse_keyed(key: String, raw: &str, today: Date) -> Result<Clause, ErrorKind> {
    let field = FieldKind::of(&key);
    let operands = lex::split(raw, ',').map(|raw| parse_operand(&key, field, raw));
    match key.as_str() {
        "is" => operands
            .map(|item| State::parse(item?.1))
            .collect::<Result<_, _>>()
            .map(Clause::Is),
        "has" => operands
            .map(|item| parse_presence(item?.1))
            .collect::<Result<_, _>>()
            .map(Clause::Has),
        _ => {
            let alternatives = operands
                .map(|item| {
                    let (operator, value) = item?;
                    let value = if field == FieldKind::Date {
                        Value::Date(
                            date::resolve(&value, today).ok_or(ErrorKind::InvalidDate { value })?,
                        )
                    } else {
                        Value::Text(value)
                    };
                    Ok(Comparison { operator, value })
                })
                .collect::<Result<_, _>>()?;
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
        match key {
            "tag" | "assignee" | "board" | "parent" | "blocks" | "blocked-by" => Self::Exact,
            "status" => Self::Status,
            "due" | "created" | "updated" => Self::Date,
            "priority" => Self::Priority,
            "is" | "has" => Self::Predicate,
            _ => Self::Custom,
        }
    }

    fn supports_comparison(self) -> bool {
        matches!(self, Self::Date | Self::Priority | Self::Custom)
    }
}

fn parse_operand(key: &str, field: FieldKind, raw: &str) -> Result<(Operator, String), ErrorKind> {
    let (operator, raw) = Operator::split(raw);
    let value = decode_required(raw, ErrorKind::EmptyValue)?;
    if operator != Operator::Eq && !field.supports_comparison() {
        return Err(ErrorKind::UnsupportedComparison { key: key.into() });
    }
    if field == FieldKind::Status
        && !matches!(value.as_str(), "todo" | "doing" | "done" | "cancelled")
    {
        return Err(ErrorKind::UnknownStatus { value });
    }
    Ok((operator, value))
}

fn decode_required(raw: &str, empty: ErrorKind) -> Result<String, ErrorKind> {
    let value = lex::decode(raw)?;
    if value.is_empty() {
        return Err(empty);
    }
    Ok(value)
}

fn error(term: &str, position: Position, kind: ErrorKind) -> ParseError {
    ParseError {
        term: term.to_owned(),
        position,
        kind,
    }
}

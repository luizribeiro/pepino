//! Parse task listing filters.

mod lex;

use std::fmt;

use thiserror::Error;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter(Vec<Term>);

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
    Is(Vec<State>),
    Has(Vec<String>),
    Match {
        key: String,
        alternatives: Vec<Comparison>,
    },
}

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

    fn parse(value: String) -> Result<Self, ErrorKind> {
        [Self::Blocked, Self::Ready, Self::Open, Self::Closed]
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or(ErrorKind::UnknownState { value })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub operator: Operator,
    pub value: String,
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
    fn split(raw: &str) -> (Self, &str) {
        [
            (Self::Le, "<="),
            (Self::Ge, ">="),
            (Self::Lt, "<"),
            (Self::Gt, ">"),
        ]
        .into_iter()
        .find_map(|(op, prefix)| raw.strip_prefix(prefix).map(|value| (op, value)))
        .unwrap_or((Self::Eq, raw))
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("filter term {term:?} at {position}: {kind}")]
pub struct ParseError {
    pub term: String,
    pub position: Position,
    pub kind: ErrorKind,
}

/// Zero-based term position in the original input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Position {
    Byte(usize),
    Argument(usize),
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Byte(index) => write!(f, "byte {index}"),
            Self::Argument(index) => write!(f, "argument {index}"),
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ErrorKind {
    #[error("unterminated quote; add a closing double quote")]
    UnterminatedQuote,
    #[error("empty term; supply a search term")]
    EmptyTerm,
    #[error("empty key; supply a field name before ':'")]
    EmptyKey,
    #[error("empty value; supply a value for each alternative")]
    EmptyValue,
    #[error("comparisons are unavailable for {key:?}; use {key}:value for equality")]
    UnsupportedComparison { key: String },
    #[error("unknown status {value:?}; use todo, doing, done, or cancelled")]
    UnknownStatus { value: String },
    #[error("unknown state {value:?}; use blocked, ready, open, or closed")]
    UnknownState { value: String },
    #[error("unsupported presence field {field:?}; use due, parent, assignee, or a custom field")]
    UnsupportedPresence { field: String },
}

pub fn parse(input: &str) -> Result<Filter, ParseError> {
    let mut terms = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        let position = Position::Byte(input.len() - rest.len());
        let end = lex::term_end(rest).map_err(|kind| error(rest, position, kind))?;
        terms.push(parse_term(&rest[..end], position)?);
        rest = rest[end..].trim_start();
    }
    Ok(Filter(terms))
}

/// Parse arguments that the caller has already split into individual terms.
pub fn parse_argv<S: AsRef<str>>(argv: &[S]) -> Result<Filter, ParseError> {
    argv.iter()
        .enumerate()
        .map(|(index, raw)| parse_term(raw.as_ref(), Position::Argument(index)))
        .collect::<Result<Vec<_>, _>>()
        .map(Filter)
}

fn parse_term(raw: &str, position: Position) -> Result<Term, ParseError> {
    let body = raw.strip_prefix('-');
    let clause = parse_clause(body.unwrap_or(raw)).map_err(|kind| error(raw, position, kind))?;
    Ok(Term {
        negated: body.is_some(),
        clause,
    })
}

fn parse_clause(body: &str) -> Result<Clause, ErrorKind> {
    if let Some((key, values)) = lex::split_once(body, ':') {
        let key = decode_required(key, ErrorKind::EmptyKey)?;
        return parse_keyed(key, values);
    }
    decode_required(body, ErrorKind::EmptyTerm).map(Clause::Text)
}

fn parse_keyed(key: String, raw: &str) -> Result<Clause, ErrorKind> {
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
                .map(|item| item.map(|(operator, value)| Comparison { operator, value }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_text_terms() {
        let filter = parse(r#"  garden "fix the \"split\" fence" -shed "#).unwrap();
        assert_eq!(
            filter
                .terms()
                .iter()
                .map(|t| (t.negated, &t.clause))
                .collect::<Vec<_>>(),
            [
                (false, &Clause::Text("garden".into())),
                (false, &Clause::Text("fix the \"split\" fence".into())),
                (true, &Clause::Text("shed".into()))
            ]
        );
        assert!(parse("  \n").unwrap().terms().is_empty());
    }

    #[test]
    fn keyed_terms_preserve_quoted_delimiters() {
        let filter =
            parse(r#"tag:garden,"needs review","tools, seeds" "plot: north" -assignee:me"#)
                .unwrap();
        assert_eq!(
            filter.terms()[0].clause,
            Clause::Match {
                key: "tag".into(),
                alternatives: ["garden", "needs review", "tools, seeds"]
                    .map(|value| Comparison {
                        operator: Operator::Eq,
                        value: value.into(),
                    })
                    .into(),
            }
        );
        assert_eq!(filter.terms()[1].clause, Clause::Text("plot: north".into()));
        assert!(filter.terms()[2].negated);
        assert_eq!(
            parse_argv(&["tag:needs review"]),
            parse(r#"tag:"needs review""#)
        );
    }

    #[test]
    fn predicates_accept_negation_and_alternatives() {
        let filter = parse(
            r#"is:blocked,ready -is:open,closed has:due,parent -has:assignee,"plant variety""#,
        )
        .unwrap();
        assert_eq!(
            filter.terms()[0].clause,
            Clause::Is(vec![State::Blocked, State::Ready])
        );
        assert_eq!(
            filter.terms()[1].clause,
            Clause::Is(vec![State::Open, State::Closed])
        );
        assert_eq!(
            filter.terms()[2].clause,
            Clause::Has(vec!["due".into(), "parent".into()])
        );
        assert_eq!(
            filter.terms()[3].clause,
            Clause::Has(vec!["assignee".into(), "plant variety".into()])
        );
        assert!(filter.terms()[1].negated && filter.terms()[3].negated);
        assert_eq!(
            parse("is:stalled").unwrap_err().kind,
            ErrorKind::UnknownState {
                value: "stalled".into()
            }
        );
        assert_eq!(
            parse("has:priority").unwrap_err().kind,
            ErrorKind::UnsupportedPresence {
                field: "priority".into()
            }
        );
        assert_eq!(
            parse("is:>open").unwrap_err().kind,
            ErrorKind::UnsupportedComparison { key: "is".into() }
        );
    }

    #[test]
    fn built_in_field_matches_preserve_symbolic_values() {
        let filter = parse("board:garden parent:ab12 blocks:cd34 blocked-by:ef56 assignee:me status:todo,doing,done,cancelled").unwrap();
        for (term, (name, value)) in filter.terms().iter().zip([
            ("board", "garden"),
            ("parent", "ab12"),
            ("blocks", "cd34"),
            ("blocked-by", "ef56"),
            ("assignee", "me"),
            ("status", "todo"),
        ]) {
            let Clause::Match { key, alternatives } = &term.clause else {
                panic!("expected match");
            };
            assert_eq!(key, name);
            assert_eq!(alternatives[0].value, value);
        }

        let error = parse("tag:garden status:todo,in-progress").unwrap_err();
        assert_eq!(error.term, "status:todo,in-progress");
        assert_eq!(
            error.kind,
            ErrorKind::UnknownStatus {
                value: "in-progress".into()
            }
        );
        assert!(
            error
                .to_string()
                .contains("todo, doing, done, or cancelled")
        );
    }

    #[test]
    fn comparison_operators_apply_to_each_alternative() {
        use Operator::*;
        let filter = parse(r#"priority:>=2,<4 effort:<=8,>12 tag:">gate""#).unwrap();
        let expected = [
            [(Ge, "2"), (Lt, "4")].as_slice(),
            &[(Le, "8"), (Gt, "12")],
            &[(Eq, ">gate")],
        ];
        for (term, pairs) in filter.terms().iter().zip(expected) {
            let Clause::Match { alternatives, .. } = &term.clause else {
                panic!("expected match");
            };
            assert_eq!(
                alternatives
                    .iter()
                    .map(|c| (c.operator, c.value.as_str()))
                    .collect::<Vec<_>>(),
                pairs
            );
        }
        let error = parse("garden -tag:>=seeds").unwrap_err();
        assert_eq!(error.term, "-tag:>=seeds");
        assert_eq!(error.position, Position::Byte(7));
        assert_eq!(
            error.kind,
            ErrorKind::UnsupportedComparison { key: "tag".into() }
        );
        assert_eq!(
            parse("priority:>=").unwrap_err().kind,
            ErrorKind::EmptyValue
        );
    }

    #[test]
    fn empty_keys_and_values_report_the_whole_term() {
        assert_eq!(parse(":garden").unwrap_err().kind, ErrorKind::EmptyKey);
        for raw in ["tag:", "tag:,garden", "tag:garden,", r#"tag:"""#] {
            let error = parse_argv(&[raw]).unwrap_err();
            assert_eq!(error.term, raw);
            assert_eq!(error.kind, ErrorKind::EmptyValue);
        }
    }

    #[test]
    fn argv_keeps_each_argument_together() {
        let argv = ["garden", "fix the fence", "-paint the shed"];
        assert_eq!(
            parse_argv(&argv),
            parse(r#"garden "fix the fence" -"paint the shed""#)
        );
        assert!(parse_argv::<&str>(&[]).unwrap().terms().is_empty());

        let error = parse_argv(&["garden", r#""fix the fence"#]).unwrap_err();
        assert_eq!(error.position, Position::Argument(1));
        assert_eq!(error.term, r#""fix the fence"#);
        assert_eq!(error.kind, ErrorKind::UnterminatedQuote);
    }

    #[test]
    fn invalid_text_terms_include_the_source() {
        for (input, kind) in [
            (r#"café "fix fence"#, ErrorKind::UnterminatedQuote),
            ("café -", ErrorKind::EmptyTerm),
        ] {
            let error = parse(input).unwrap_err();
            assert_eq!(error.position, Position::Byte(6));
            assert_eq!(error.term, input[6..]);
            assert_eq!(error.kind, kind);
        }
    }
}

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
    Match {
        key: String,
        alternatives: Vec<Comparison>,
    },
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
        let alternatives = lex::split(values, ',')
            .map(|value| parse_comparison(&key, value))
            .collect::<Result<_, _>>()?;
        return Ok(Clause::Match { key, alternatives });
    }
    decode_required(body, ErrorKind::EmptyTerm).map(Clause::Text)
}

fn parse_comparison(key: &str, raw: &str) -> Result<Comparison, ErrorKind> {
    let (operator, raw) = Operator::split(raw);
    let value = decode_required(raw, ErrorKind::EmptyValue)?;
    if operator != Operator::Eq
        && matches!(
            key,
            "tag"
                | "status"
                | "assignee"
                | "board"
                | "parent"
                | "blocks"
                | "blocked-by"
                | "is"
                | "has"
        )
    {
        return Err(ErrorKind::UnsupportedComparison { key: key.into() });
    }
    Ok(Comparison { operator, value })
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

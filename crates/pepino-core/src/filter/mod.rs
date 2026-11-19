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
    pub text: String,
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
    let text = lex::decode(body.unwrap_or(raw)).map_err(|kind| error(raw, position, kind))?;
    if text.is_empty() {
        return Err(error(raw, position, ErrorKind::EmptyTerm));
    }
    Ok(Term {
        negated: body.is_some(),
        text,
    })
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
                .map(|t| (t.negated, t.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (false, "garden"),
                (false, "fix the \"split\" fence"),
                (true, "shed")
            ]
        );
        assert!(parse("  \n").unwrap().terms().is_empty());
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

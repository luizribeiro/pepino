//! Parse task listing filters.

mod lex;

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
#[error("filter term {term:?} at byte {position}: {kind}")]
pub struct ParseError {
    pub term: String,
    pub position: usize,
    pub kind: ErrorKind,
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
        let position = input.len() - rest.len();
        let error = |term: &str, kind| ParseError {
            term: term.to_owned(),
            position,
            kind,
        };
        let end = lex::term_end(rest).map_err(|kind| error(rest, kind))?;
        let raw = &rest[..end];
        let negated = raw.starts_with('-');
        let text = lex::decode(raw.strip_prefix('-').unwrap_or(raw));
        if text.is_empty() {
            return Err(error(raw, ErrorKind::EmptyTerm));
        }
        terms.push(Term { negated, text });
        rest = rest[end..].trim_start();
    }
    Ok(Filter(terms))
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
    fn invalid_text_terms_include_the_source() {
        for (input, kind) in [
            (r#"café "fix fence"#, ErrorKind::UnterminatedQuote),
            ("café -", ErrorKind::EmptyTerm),
        ] {
            let error = parse(input).unwrap_err();
            assert_eq!(error.position, 6);
            assert_eq!(error.term, input[6..]);
            assert_eq!(error.kind, kind);
        }
    }
}

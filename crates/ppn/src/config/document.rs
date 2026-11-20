use std::ops::Range;
use std::path::Path;
use std::str::FromStr;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::ValueError;
use super::diagnostics::{at_location, nearest_key};

pub(crate) type Input<'a> = Spanned<DeValue<'a>>;

pub(crate) struct Validator<'a> {
    pub problems: Vec<String>,
    pub file: Option<(&'a Path, &'a str)>,
}

impl Validator<'_> {
    pub fn report(
        &mut self,
        key: &str,
        span: Option<Range<usize>>,
        message: impl std::fmt::Display,
    ) {
        let location = self.file.map(|(path, text)| at_location(path, text, span));
        self.problems.push(match location {
            Some(location) => format!("{location}: {key}: {message}"),
            None => format!("{key}: {message}"),
        });
    }

    pub fn require<T>(
        &mut self,
        key: &str,
        value: &Input<'_>,
        result: Result<T, ValueError>,
    ) -> Option<T> {
        result
            .map_err(|error| self.report(key, Some(value.span()), error))
            .ok()
    }

    pub fn parse<T: FromStr<Err = ValueError>>(
        &mut self,
        key: &str,
        input: &Input<'_>,
    ) -> Option<T> {
        self.require(key, input, parse_value(input.get_ref()))
    }

    pub fn list<T>(
        &mut self,
        key: &str,
        input: &Input<'_>,
        mut parse: impl FnMut(&mut Self, &str, &Input<'_>) -> Option<T>,
    ) -> Option<Vec<T>> {
        let array = input
            .get_ref()
            .as_array()
            .ok_or(ValueError("expected an array"));
        let array = self.require(key, input, array)?;
        let values = array
            .iter()
            .enumerate()
            .filter_map(|(index, input)| parse(self, &format!("{key}[{index}]"), input))
            .collect::<Vec<_>>();
        (values.len() == array.len()).then_some(values)
    }

    pub fn unknown(&mut self, key: &str, span: Range<usize>, keys: &[&str]) {
        let suggestion = nearest_key(key, keys.iter().copied());
        let message = match suggestion {
            Some(suggestion) => format!("unknown key; did you mean `{suggestion}`?"),
            None => "unknown key".into(),
        };
        self.report(key, Some(span), message);
    }

    pub fn document<'a>(&mut self, text: &'a str) -> DeTable<'a> {
        let (table, problems) = DeTable::parse_recoverable(text);
        for problem in problems {
            self.report("TOML", problem.span(), problem.message());
        }
        table.into_inner()
    }
}

pub(crate) fn parse_value<T: FromStr<Err = ValueError>>(
    value: &DeValue<'_>,
) -> Result<T, ValueError> {
    text(value)?.parse()
}

pub(crate) fn text<'a>(value: &'a DeValue<'_>) -> Result<&'a str, ValueError> {
    value.as_str().ok_or(ValueError("expected a string"))
}

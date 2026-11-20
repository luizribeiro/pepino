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
    file: Option<(&'a Path, &'a str)>,
    syntax_offsets: Vec<usize>,
}

impl<'a> Validator<'a> {
    pub fn new(file: Option<(&'a Path, &'a str)>) -> Self {
        Self {
            problems: Vec::new(),
            file,
            syntax_offsets: Vec::new(),
        }
    }

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
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                let span = value.span();
                if !self.syntax_offsets.contains(&span.start) {
                    self.report(key, Some(span), error);
                }
                None
            }
        }
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

    pub fn unknown<'key>(
        &mut self,
        key: &str,
        span: Range<usize>,
        keys: impl IntoIterator<Item = &'key str>,
    ) {
        let suggestion = nearest_key(key, keys);

        let message = match suggestion {
            Some(suggestion) => format!("unknown key; did you mean `{suggestion}`?"),
            None => "unknown key".into(),
        };
        self.report(key, Some(span), message);
    }

    pub fn document(&mut self, text: &'a str) -> DeTable<'a> {
        let (table, problems) = DeTable::parse_recoverable(text);
        let table = table.into_inner();
        for problem in problems {
            let span = problem.span();
            let key = span.as_ref().and_then(|span| {
                self.syntax_offsets.push(span.start);
                key_at(&table, span.start).or_else(|| key_on_line(text, span.start))
            });
            self.report(key.as_deref().unwrap_or("TOML"), span, problem.message());
        }
        table
    }
}

fn key_at(table: &DeTable<'_>, offset: usize) -> Option<String> {
    table.iter().find_map(|(key, input)| {
        let name = key.get_ref();
        value_suffix_at(input, offset)
            .map(|suffix| format!("{name}{suffix}"))
            .or_else(|| key.span().contains(&offset).then(|| name.to_string()))
    })
}

fn value_suffix_at(input: &Input<'_>, offset: usize) -> Option<String> {
    let suffix = match input.get_ref() {
        DeValue::Table(table) => key_at(table, offset).map(|key| format!(".{key}")),
        DeValue::Array(items) => items.iter().enumerate().find_map(|(index, input)| {
            value_suffix_at(input, offset).map(|suffix| format!("[{index}]{suffix}"))
        }),
        _ => None,
    };
    let span = input.span();
    suffix.or_else(|| (span.start == offset || span.contains(&offset)).then(String::new))
}

fn key_on_line(text: &str, offset: usize) -> Option<String> {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let before = text[..offset].rsplit('\n').next()?;
    let after = text[offset..].split('\n').next()?;
    let line = &text[offset - before.len()..offset + after.len()];
    if !line.contains('=') {
        return None;
    }
    let (table, _) = DeTable::parse_recoverable(line);
    table
        .into_inner()
        .into_iter()
        .next()
        .map(|(key, _)| key.into_inner().into_owned())
}

pub(crate) fn parse_value<T: FromStr<Err = ValueError>>(
    value: &DeValue<'_>,
) -> Result<T, ValueError> {
    text(value)?.parse()
}

pub(crate) fn text<'a>(value: &'a DeValue<'_>) -> Result<&'a str, ValueError> {
    value.as_str().ok_or(ValueError("expected a string"))
}

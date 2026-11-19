use super::ErrorKind;

pub(super) fn characters(
    input: &str,
) -> impl Iterator<Item = Result<(usize, char, bool), ErrorKind>> {
    let mut chars = input.char_indices().peekable();
    let mut quoted = false;
    std::iter::from_fn(move || {
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '"' => quoted = !quoted,
                '\\' if quoted
                    && chars
                        .peek()
                        .is_some_and(|&(_, next)| matches!(next, '"' | '\\')) =>
                {
                    return chars.next().map(|(_, escaped)| Ok((offset, escaped, true)));
                }
                _ => return Some(Ok((offset, ch, quoted))),
            }
        }
        std::mem::take(&mut quoted).then_some(Err(ErrorKind::UnterminatedQuote))
    })
}

pub(super) fn term_end(input: &str) -> Result<usize, ErrorKind> {
    for item in characters(input) {
        let (offset, ch, quoted) = item?;
        if !quoted && ch.is_whitespace() {
            return Ok(offset);
        }
    }
    Ok(input.len())
}

pub(super) fn decode(input: &str) -> Result<String, ErrorKind> {
    characters(input)
        .map(|item| item.map(|(_, ch, _)| ch))
        .collect()
}

pub(super) fn split_once(input: &str, delimiter: char) -> Option<(&str, &str)> {
    characters(input).find_map(|item| {
        let (offset, ch, quoted) = item.ok()?;
        (!quoted && ch == delimiter).then(|| (&input[..offset], &input[offset + ch.len_utf8()..]))
    })
}

pub(super) fn split(input: &str, delimiter: char) -> impl Iterator<Item = &str> {
    let mut rest = Some(input);
    std::iter::from_fn(move || {
        let input = rest.take()?;
        if let Some((head, tail)) = split_once(input, delimiter) {
            rest = Some(tail);
            return Some(head);
        }
        Some(input)
    })
}

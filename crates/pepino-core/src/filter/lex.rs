pub(super) fn characters(input: &str) -> impl Iterator<Item = Result<(usize, char, bool), usize>> {
    let mut chars = input.char_indices().peekable();
    let mut opening = None;
    std::iter::from_fn(move || {
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '"' => {
                    opening = if opening.is_some() {
                        None
                    } else {
                        Some(offset)
                    };
                }
                '\\' if opening.is_some()
                    && chars
                        .peek()
                        .is_some_and(|&(_, next)| matches!(next, '"' | '\\')) =>
                {
                    return chars.next().map(|(_, escaped)| Ok((offset, escaped, true)));
                }
                _ => return Some(Ok((offset, ch, opening.is_some()))),
            }
        }
        opening.take().map(Err)
    })
}

pub(super) fn term_end(input: &str) -> Result<usize, usize> {
    for item in characters(input) {
        let (offset, ch, quoted) = item?;
        if !quoted && ch.is_whitespace() {
            return Ok(offset);
        }
    }
    Ok(input.len())
}

pub(super) fn decode(input: &str) -> Result<String, usize> {
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

pub(super) fn split(input: &str, delimiter: char) -> impl Iterator<Item = (usize, &str)> {
    let mut rest = Some(input);
    std::iter::from_fn(move || {
        let remaining = rest.take()?;
        let offset = input.len() - remaining.len();
        if let Some((head, tail)) = split_once(remaining, delimiter) {
            rest = Some(tail);
            return Some((offset, head));
        }
        Some((offset, remaining))
    })
}

pub(super) fn ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

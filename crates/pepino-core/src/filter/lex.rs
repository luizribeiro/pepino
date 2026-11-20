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
                '\\' if opening.is_some() => {
                    if chars
                        .peek()
                        .is_some_and(|&(_, next)| matches!(next, '"' | '\\'))
                    {
                        return chars.next().map(|(_, escaped)| Ok((offset, escaped, true)));
                    }
                    if let Some((space, size)) = whitespace_escape(&input[offset + 1..]) {
                        chars.by_ref().take(size).for_each(drop);
                        return Some(Ok((offset, space, true)));
                    }
                    return Some(Ok((offset, ch, true)));
                }
                _ => return Some(Ok((offset, ch, opening.is_some()))),
            }
        }
        opening.take().map(Err)
    })
}

fn whitespace_escape(input: &str) -> Option<(char, usize)> {
    let digits = input.strip_prefix("u{")?;
    let end = digits.bytes().take(7).position(|byte| byte == b'}')?;
    let digits = digits.get(..end)?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let scalar = u32::from_str_radix(digits, 16).ok()?;
    let space = char::from_u32(scalar)?;
    space.is_whitespace().then_some((space, end + 3))
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

use std::collections::BTreeMap;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn assignee_label(id: &str, names: &BTreeMap<String, String>) -> String {
    single_line(
        names
            .get(id)
            .map_or_else(|| super::short_id(id), String::as_str),
    )
}

pub fn single_line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

pub fn truncate_title(title: &str, width: usize) -> String {
    if title.width() <= width {
        return title.into();
    }
    let Some(mut remaining) = width.checked_sub(1) else {
        return String::new();
    };
    let mut shortened = String::new();
    for grapheme in title.graphemes(true) {
        let Some(left) = remaining.checked_sub(grapheme.width()) else {
            break;
        };
        shortened.push_str(grapheme);
        remaining = left;
    }
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_known_and_unknown_assignees() {
        let names = BTreeMap::from([("a6e1a65f".into(), "Ada Gardener".into())]);
        assert_eq!(assignee_label("a6e1a65f", &names), "Ada Gardener");
        assert_eq!(assignee_label("6b376b12", &names), "6b376b");
    }

    #[test]
    fn preserves_screen_columns_and_grapheme_boundaries() {
        assert_eq!(
            single_line("Fix\tthe latch\nCheck\x1b"),
            "Fix the latch Check "
        );
        assert_eq!(truncate_title("修理 the latch", 4), "修…");
        assert_eq!(truncate_title("Cafe\u{301} sign", 5), "Cafe\u{301}…");
        assert_eq!(truncate_title("Repair the latch", 0), "");
        assert_eq!(truncate_title("Repair the latch", 16), "Repair the latch");
    }
}

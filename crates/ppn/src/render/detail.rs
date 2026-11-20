use std::collections::BTreeMap;
use std::fmt::Write;

use pepino_app::{EdgeEffect, TaskRecord};

use super::detail_fields::{indented, task_timestamps};
use super::row::task_line;
use super::{Dates, ShortIds, assignee_label, task_fields, task_relations};
use crate::terminal::Terminal;

pub fn task_detail(
    task: &TaskRecord,
    edges: &[EdgeEffect],
    titles: &BTreeMap<String, String>,
    names: &BTreeMap<String, String>,
    dates: Dates<'_>,
    terminal: &Terminal,
) -> String {
    let ids = ShortIds::new(
        std::iter::once(&task.id)
            .chain(task.parent.iter())
            .chain(&task.children)
            .chain(&task.blocks)
            .chain(&task.blocked_by)
            .chain(&task.relates)
            .chain(edges.iter().flat_map(|edge| [&edge.from, &edge.to]))
            .map(String::as_str),
    );
    let mut text = task_line(task, &ids, Vec::new(), terminal);
    text.push('\n');
    text.push_str(&task_fields(task, names, dates, terminal));
    text.push_str(&task_relations(task, edges, &ids, titles));
    text.push_str(&task_timestamps(task, names, dates, terminal));
    if !task.comments.is_empty() {
        let _ = writeln!(text, "\nComments ({})", task.comment_count);
    }
    for comment in &task.comments {
        let _ = writeln!(
            text,
            "  {} · {}",
            assignee_label(&comment.author, names),
            dates.relative_time(comment.hlc, terminal)
        );
        text.push_str(&indented(&comment.text, "    "));
    }
    text
}

#[cfg(test)]
mod tests {
    use pepino_app::{CommentRecord, EdgeExclusionReason, EdgeKind};

    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn detail_golden_text_on_a_fixed_width_terminal() {
        let summary = task();
        let peer = "97ad97ff";
        let edges = [EdgeEffect {
            from: summary.id.clone(),
            kind: EdgeKind::Blocks,
            to: peer.into(),
            effective: false,
            reason: Some(EdgeExclusionReason::Cycle),
        }];
        let comments = vec![CommentRecord {
            id: "c201ef42".into(),
            author: summary.author.clone(),
            hlc: summary.updated,
            text: "The new latch arrived.\nUse the spare screw.".into(),
        }];
        let task = TaskRecord { summary, comments };
        let titles = BTreeMap::from([(peer.into(), "Order a replacement screw".into())]);
        let names = BTreeMap::from([
            ("a6e1a65f".into(), "Ada Gardener".into()),
            ("6b376b12".into(), "Ada".into()),
        ]);
        let render = |color| {
            task_detail(
                &task,
                &edges,
                &titles,
                &names,
                dates(),
                &Terminal::fixed(Some(75), color),
            )
        };
        let expected = include_str!("detail.txt");
        assert_eq!(render(false), expected);
        assert_eq!(
            render(true),
            expected
                .replacen("97ad97a", "\x1b[36m97ad97a\x1b[0m", 1)
                .replacen("todo · blocked", "\x1b[33mtodo · blocked\x1b[0m", 1)
        );
    }
}

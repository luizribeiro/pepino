use std::collections::BTreeMap;
use std::fmt::Write;

use clap::builder::styling::AnsiColor;
use pepino_app::{EdgeEffect, TaskRecord};

use super::detail_fields::indented;
use super::{Dates, ShortIds, short_id, single_line, task_fields, task_relations};
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
    let id = terminal.paint(ids.get(&task.id), AnsiColor::Cyan);
    let title = single_line(&task.title);
    let mut text = format!("{id}  {title}\n");
    text.push_str(&task_fields(task, names, dates, terminal));
    text.push_str(&task_relations(task, edges, &ids, titles));
    let _ = writeln!(text, "Comments ({}):", task.comment_count);
    for comment in &task.comments {
        let _ = writeln!(
            text,
            "  {} by {} at {}",
            short_id(&comment.id),
            short_id(&comment.author),
            dates.timestamp(comment.hlc)
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
        let names = BTreeMap::from([("a6e1a65f".into(), "Ada Gardener".into())]);
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
            expected.replacen("97ad97a", "\x1b[36m97ad97a\x1b[0m", 1)
        );
    }
}

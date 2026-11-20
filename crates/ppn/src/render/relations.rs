use std::collections::BTreeMap;
use std::fmt::Write;

use pepino_app::{EdgeEffect, EdgeExclusionReason, EdgeKind, TaskSummary};

use super::{ShortIds, single_line};

pub fn edge_reason(edge: &EdgeEffect) -> Option<&'static str> {
    if edge.effective {
        return None;
    }
    Some(match edge.reason? {
        EdgeExclusionReason::MissingEndpoint => "an endpoint is missing",
        EdgeExclusionReason::SupersededParent => "a newer parent link takes precedence",
        EdgeExclusionReason::Cycle => "it would create a cycle",
        _ => "the app excluded this link",
    })
}

fn label(id: &str, ids: &ShortIds<'_>, titles: &BTreeMap<String, String>) -> String {
    match titles.get(id) {
        Some(title) => format!("{}  {}", ids.get(id), single_line(title)),
        None => ids.get(id).to_owned(),
    }
}

pub fn link_description(
    edge: &EdgeEffect,
    ids: &ShortIds<'_>,
    titles: &BTreeMap<String, String>,
) -> String {
    let relation = match edge.kind {
        EdgeKind::Parent => "has parent",
        EdgeKind::Blocks => "blocks",
        EdgeKind::Relates => "relates to",
        _ => "links to",
    };
    format!(
        "{} {relation} {}",
        label(&edge.from, ids, titles),
        label(&edge.to, ids, titles)
    )
}

pub fn task_relations(
    task: &TaskSummary,
    edges: &[EdgeEffect],
    ids: &ShortIds<'_>,
    titles: &BTreeMap<String, String>,
) -> String {
    let mut output = String::new();
    let groups = [
        ("Parent", task.parent.iter().collect::<Vec<_>>()),
        ("Children", task.children.iter().collect()),
        ("Blocks", task.blocks.iter().collect()),
        ("Blocked by", task.blocked_by.iter().collect()),
        ("Relates", task.relates.iter().collect()),
    ];
    for (name, related) in groups {
        let labels = related
            .into_iter()
            .map(|id| label(id, ids, titles))
            .collect::<Vec<_>>();
        if labels.is_empty() {
            let _ = writeln!(output, "{name}: -");
        } else {
            let _ = writeln!(output, "{name}: {}", labels.join(", "));
        }
    }
    for (edge, reason) in edges
        .iter()
        .filter_map(|edge| Some((edge, edge_reason(edge)?)))
    {
        let _ = writeln!(
            output,
            "Excluded link: {} ({reason})",
            link_description(edge, ids, titles)
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_excluded_edges_and_keeps_unlinked_edges_quiet() {
        let mut edge = EdgeEffect {
            from: "97ad97ab".into(),
            kind: EdgeKind::Blocks,
            to: "97ad97ff".into(),
            effective: false,
            reason: Some(EdgeExclusionReason::Cycle),
        };
        let ids = ShortIds::new([edge.from.as_str(), edge.to.as_str()]);
        let titles = BTreeMap::from([(edge.to.clone(), "Order a replacement screw".into())]);
        assert_eq!(
            link_description(&edge, &ids, &titles),
            "97ad97a blocks 97ad97f  Order a replacement screw"
        );
        assert_eq!(edge_reason(&edge), Some("it would create a cycle"));
        edge.reason = None;
        assert_eq!(edge_reason(&edge), None);
    }
}

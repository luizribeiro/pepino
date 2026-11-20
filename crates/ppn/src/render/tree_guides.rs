pub(super) fn tree_guides(depths: &[usize]) -> Vec<String> {
    let mut branch = Vec::new();
    let later_siblings: Vec<_> = depths
        .iter()
        .rev()
        .map(|&depth| {
            branch.resize(depth + 1, false);
            std::mem::replace(&mut branch[depth], true)
        })
        .collect();

    branch.clear();
    depths
        .iter()
        .zip(later_siblings.into_iter().rev())
        .map(|(&depth, later)| {
            branch.truncate(depth);
            let mut prefix: String = branch
                .iter()
                .skip(1)
                .map(|&continues| if continues { "│   " } else { "    " })
                .collect();
            if depth != 0 {
                prefix.push_str(if later { "├── " } else { "└── " });
            }
            branch.push(later);
            prefix
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tree_guides;

    #[test]
    fn tree_guides_keep_ancestor_siblings_within_their_branch() {
        assert_eq!(
            tree_guides(&[0, 1, 2, 3, 2, 1, 2, 0, 1]),
            [
                "",
                "├── ",
                "│   ├── ",
                "│   │   └── ",
                "│   └── ",
                "└── ",
                "    └── ",
                "",
                "└── "
            ]
        );
    }
}

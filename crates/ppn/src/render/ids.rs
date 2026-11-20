pub fn short_id(id: &str) -> &str {
    id.get(..6).unwrap_or(id)
}

pub struct ShortIds<'a>(Vec<&'a str>);

impl<'a> ShortIds<'a> {
    pub fn new(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        Self(ids)
    }

    pub fn get<'b>(&self, id: &'b str) -> &'b str {
        let Ok(index) = self.0.binary_search(&id) else {
            return short_id(id);
        };
        let neighbor = |other: &&str| {
            id.bytes()
                .zip(other.bytes())
                .take_while(|(a, b)| a == b)
                .count()
                + 1
        };
        let previous = index.checked_sub(1).and_then(|index| self.0.get(index));
        let next = self.0.get(index + 1);
        let length = previous
            .into_iter()
            .chain(next)
            .map(neighbor)
            .max()
            .unwrap_or(6)
            .max(6);
        id.get(..length).unwrap_or(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_shortest_prefix_unique_among_visible_tasks() {
        let ids = ShortIds::new([
            "97ad010ff",
            "97ad0abff",
            "97ad010e0",
            "33ea33ff",
            "97ad010ff",
        ]);
        assert_eq!(ids.get("97ad010ff"), "97ad010f");
        assert_eq!(ids.get("97ad010e0"), "97ad010e");
        assert_eq!(ids.get("97ad0abff"), "97ad0a");
        assert_eq!(ids.get("33ea33ff"), "33ea33");
        assert_eq!(ids.get("4e514ba0"), "4e514b");
    }
}

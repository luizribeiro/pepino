use pepino_core::{EdgeKind, TaskId};

use crate::StoreError;

pub(crate) const PARENT: u8 = 0;
pub(crate) const BLOCKS: u8 = 1;
pub(crate) const RELATES: u8 = 2;

pub(crate) fn member(kind: EdgeKind, to: TaskId) -> Result<[u8; 33], StoreError> {
    let mut member = [0; 33];
    member[0] = match kind {
        EdgeKind::Parent => PARENT,
        EdgeKind::Blocks => BLOCKS,
        EdgeKind::Relates => RELATES,
    };
    member[1..].copy_from_slice(to.as_bytes());
    Ok(member)
}

pub(crate) fn decode(member: [u8; 33]) -> Result<(EdgeKind, TaskId), StoreError> {
    let [kind, to @ ..] = member;
    let kind = match kind {
        PARENT => EdgeKind::Parent,
        BLOCKS => EdgeKind::Blocks,
        RELATES => EdgeKind::Relates,
        _ => return Err(StoreError::ViewValue("invalid edge kind".into())),
    };
    Ok((kind, TaskId::from_bytes(to)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_every_edge_kind_and_preserves_all_destination_bytes() {
        let to = TaskId::from_bytes(std::array::from_fn(|index| 31 + index as u8));
        for kind in [EdgeKind::Parent, EdgeKind::Blocks, EdgeKind::Relates] {
            assert_eq!(decode(member(kind, to).unwrap()).unwrap(), (kind, to));
        }
        let mut invalid = member(EdgeKind::Parent, to).unwrap();
        invalid[0] = 255;
        assert!(matches!(decode(invalid), Err(StoreError::ViewValue(_))));
    }
}

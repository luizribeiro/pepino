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
        _ => return Err(StoreError::ViewValue("unsupported edge kind".into())),
    };
    member[1..].copy_from_slice(to.as_bytes());
    Ok(member)
}

pub(crate) fn decode(member: [u8; 33]) -> Result<(EdgeKind, TaskId), StoreError> {
    let to = member.last_chunk::<32>().expect("33-byte edge key");
    let kind = match member[0] {
        PARENT => EdgeKind::Parent,
        BLOCKS => EdgeKind::Blocks,
        RELATES => EdgeKind::Relates,
        _ => return Err(StoreError::ViewValue("invalid edge kind".into())),
    };
    Ok((kind, TaskId::from_bytes(*to)))
}

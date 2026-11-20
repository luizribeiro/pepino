use p2panda::{AccessLevel, spaces::Member};
use pepino_core::DeviceId;

use crate::{BoardHandle, Error, Node};

pub type MemberInfo = [u8; 305];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Role {
    Pull,
    Read,
    Write,
    Manage,
}

impl Role {
    fn level(self) -> AccessLevel {
        match self {
            Self::Pull => AccessLevel::Pull,
            Self::Read => AccessLevel::Read,
            Self::Write => AccessLevel::Write,
            Self::Manage => AccessLevel::Manage,
        }
    }

    fn from_level(level: AccessLevel) -> Self {
        match level {
            AccessLevel::Pull => Self::Pull,
            AccessLevel::Read => Self::Read,
            AccessLevel::Write => Self::Write,
            AccessLevel::Manage => Self::Manage,
        }
    }
}

impl Node {
    pub async fn member_info(&self) -> Result<MemberInfo, Error> {
        let node = self.inner.clone();
        self.worker
            .run(async move { Ok(node.me().await.map_err(Error::node)?.to_bytes()) })
            .await
    }
}

impl BoardHandle {
    pub async fn add_member(&self, info: &MemberInfo, role: Role) -> Result<(), Error> {
        let member = Member::from_bytes(info).map_err(|error| Error::Member(error.to_string()))?;
        member
            .verify()
            .map_err(|error| Error::Member(error.to_string()))?;
        let id = member.id();
        let space = self.space()?;
        let node = self.node.upgrade().ok_or(Error::Closed)?;
        self.run(async move {
            node.register_member(member).await.map_err(Error::node)?;
            space.add(id, role.level()).await.map_err(Error::node)
        })
        .await
    }

    pub async fn members(&self) -> Result<Vec<(DeviceId, Role)>, Error> {
        let space = self.space()?;
        self.run(async move {
            let mut members: Vec<_> = space
                .members()
                .await
                .map_err(Error::node)?
                .into_iter()
                .map(|(id, role)| (DeviceId::from_bytes(*id.as_bytes()), Role::from_level(role)))
                .collect();
            members.sort_unstable_by_key(|(device, _)| *device);
            Ok(members)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_round_trip_including_ciphertext_only_access() {
        for role in [Role::Pull, Role::Read, Role::Write, Role::Manage] {
            assert_eq!(Role::from_level(role.level()), role);
        }
    }
}

use p2panda::{AccessLevel, spaces::MemberId};
use pepino_core::DeviceId;

use crate::{BoardHandle, Error, Node};

#[path = "member_info.rs"]
mod member_info;
pub use member_info::MemberInfo;

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
            .run(async move {
                MemberInfo::from_bytes(&node.me().await.map_err(Error::node)?.to_bytes())
            })
            .await
    }
}

impl BoardHandle {
    pub async fn add_member(&self, info: &MemberInfo, role: Role) -> Result<(), Error> {
        let member = info.member()?;
        let id = member.id();
        let space = self.space_for_management()?;
        let node = self.node.upgrade().ok_or(Error::Closed)?;
        let lock = self.membership_lock.clone();
        self.run(async move {
            let _guard = lock.lock().await;
            let actors = space.actors().await.map_err(Error::node)?;
            require_manager(&actors, node.id())?;
            match actors.into_iter().find(|(actor, _)| *actor == id) {
                Some((_, level)) if level == role.level() => Ok(()),
                Some(_) => Err(Error::Member(
                    "device already has a different direct role".into(),
                )),
                None => {
                    node.register_member(member).await.map_err(Error::node)?;
                    space.add(id, role.level()).await.map_err(Error::node)
                }
            }
        })
        .await
    }

    fn space_for_management(
        &self,
    ) -> Result<std::sync::Arc<p2panda::spaces::Space<crate::receipt::Payload>>, Error> {
        self.space().map_err(|error| match error {
            Error::NotJoined => Error::NotPermitted,
            other => other,
        })
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

fn require_manager(actors: &[(MemberId, AccessLevel)], device: MemberId) -> Result<(), Error> {
    if actors.contains(&(device, AccessLevel::Manage)) {
        Ok(())
    } else {
        Err(Error::NotPermitted)
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

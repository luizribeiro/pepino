use std::collections::BTreeMap;

use p2panda::{AccessLevel, Hash, operation::Extensions, spaces::MemberId};
use p2panda_spaces::SpacesStoreState;
use p2panda_store::{
    SqliteStore,
    spaces::{SpacesStore, SqliteSpacesStore},
};
use pepino_core::{BoardId, DeviceId};
use sqlx::SqlitePool;

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

/// The state of a device in the board's locally materialized membership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MemberState {
    Member(Role),
    Removed,
    NotYetAdded,
}

/// Current members and devices recorded by p2panda as removed.
pub type MemberStates = BTreeMap<DeviceId, MemberState>;

pub(crate) async fn read_members(pool: SqlitePool, board: BoardId) -> Result<MemberStates, Error> {
    let store = SqliteSpacesStore::<Extensions>::new(SqliteStore::from_pool(pool));
    let state: Option<SpacesStoreState<()>> = store
        .get_space_state(&Hash::from_bytes(*board.as_bytes()))
        .await
        .map_err(Error::storage)?;
    let mut members = MemberStates::new();
    if let Some(state) = state {
        for device in state.removed {
            members.insert(device_id(device), MemberState::Removed);
        }
        for (device, access) in state.groups_y.members(state.group_id) {
            members.insert(
                device_id(device),
                MemberState::Member(Role::from_level(access.level)),
            );
        }
    }
    Ok(members)
}

fn device_id(member: MemberId) -> DeviceId {
    DeviceId::from_bytes(*member.as_bytes())
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

    pub(crate) fn from_level(level: AccessLevel) -> Self {
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

    pub async fn set_role(&self, member: DeviceId, role: Role) -> Result<(), Error> {
        let space = self.space_for_management()?;
        let device = self.node.upgrade().ok_or(Error::Closed)?.id();
        let lock = self.membership_lock.clone();
        self.run(async move {
            let _guard = lock.lock().await;
            let actors = space.actors().await.map_err(Error::node)?;
            require_manager(&actors, device)?;
            let (member, current) = find_actor(&actors, member).ok_or(Error::NotMember)?;
            let level = role.level();
            match level.cmp(&current) {
                std::cmp::Ordering::Greater => {
                    space.promote(member, level).await.map_err(Error::node)
                }
                std::cmp::Ordering::Less => {
                    keep_manager(&actors, current)?;
                    space.demote(member, level).await.map_err(Error::node)
                }
                std::cmp::Ordering::Equal => Ok(()),
            }
        })
        .await
    }

    /// Remove a direct member and rotate the board's encryption keys. The removed
    /// device keeps everything it already decrypted and can still receive ciphertext.
    ///
    /// Removing another manager requires `allow_manager_removal`. Concurrent mutual
    /// removal by managers can leave the board unrecoverable.
    pub async fn remove_member(
        &self,
        member: DeviceId,
        allow_manager_removal: bool,
    ) -> Result<(), Error> {
        let space = self.space_for_management()?;
        let device = self.node.upgrade().ok_or(Error::Closed)?.id();
        let lock = self.membership_lock.clone();
        self.run(async move {
            let _guard = lock.lock().await;
            let actors = space.actors().await.map_err(Error::node)?;
            require_manager(&actors, device)?;
            let (member, role) = find_actor(&actors, member).ok_or(Error::NotMember)?;
            keep_manager(&actors, role)?;
            if role == AccessLevel::Manage && member != device && !allow_manager_removal {
                return Err(Error::ManagerRemovalRequiresConfirmation);
            }
            space.remove(member).await.map_err(Error::node)
        })
        .await
    }

    /// Leave the board. p2panda 0.8.1 requires manager access for self-removal.
    pub async fn leave(&self) -> Result<(), Error> {
        let device = self.node.upgrade().ok_or(Error::Closed)?.id();
        self.remove_member(device_id(device), false).await
    }

    fn space_for_management(
        &self,
    ) -> Result<std::sync::Arc<p2panda::spaces::Space<crate::receipt::Payload>>, Error> {
        self.space().map_err(|error| match error {
            Error::NotJoined => Error::NotPermitted,
            other => other,
        })
    }

    /// Read the current membership, including removed devices.
    pub async fn member_states(&self) -> Result<MemberStates, Error> {
        self.run(read_members(self.membership_pool.clone(), self.id()))
            .await
    }

    /// Read this device's state, including before it joins or after it is removed.
    pub async fn member_state(&self) -> Result<MemberState, Error> {
        let id = self.node.upgrade().ok_or(Error::Closed)?.id();
        Ok(self
            .member_states()
            .await?
            .get(&device_id(id))
            .copied()
            .unwrap_or(MemberState::NotYetAdded))
    }

    /// List the active members and their roles.
    pub async fn members(&self) -> Result<Vec<(DeviceId, Role)>, Error> {
        let space = self.space()?;
        self.run(async move {
            let mut members: Vec<_> = space
                .members()
                .await
                .map_err(Error::node)?
                .into_iter()
                .map(|(id, role)| (device_id(id), Role::from_level(role)))
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

fn find_actor(
    actors: &[(MemberId, AccessLevel)],
    device: DeviceId,
) -> Option<(MemberId, AccessLevel)> {
    actors
        .iter()
        .copied()
        .find(|(actor, _)| actor.as_bytes() == device.as_bytes())
}

fn keep_manager(actors: &[(MemberId, AccessLevel)], current: AccessLevel) -> Result<(), Error> {
    if current == AccessLevel::Manage
        && actors
            .iter()
            .filter(|(_, role)| *role == AccessLevel::Manage)
            .count()
            == 1
    {
        return Err(Error::LastManager);
    }
    Ok(())
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

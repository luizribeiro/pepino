use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use p2panda::{
    AccessLevel, Hash,
    operation::{Extensions, ExtensionsVariantV1, Operation, SpacesHeaderArgs},
    spaces::MemberId,
};
use p2panda_core::traits::Provenance;
use p2panda_spaces::SpacesStoreState;
use p2panda_store::{
    SqliteStore,
    operations::OperationStore,
    spaces::{SpacesStore, SqliteSpacesStore},
};
use pepino_core::{BoardId, DeviceId, EventId};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

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
    /// Read the last processed board membership without opening a network node.
    pub async fn persisted_members(path: &Path, board: BoardId) -> Result<MemberStates, Error> {
        let options = SqliteConnectOptions::new().filename(path).read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(Error::storage)?;
        let members = read_members(pool.clone(), board).await;
        pool.close().await;
        members
    }

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
        match self.enroll_direct_member(info, role).await {
            Err(Error::DifferentDirectRole) => {
                Err(Error::Member(Error::DifferentDirectRole.to_string()))
            }
            result => result,
        }
    }

    /// Enroll a direct member with a distinct error for an existing different role.
    pub async fn enroll_direct_member(&self, info: &MemberInfo, role: Role) -> Result<(), Error> {
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
                Some(_) => Err(Error::DifferentDirectRole),
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

type AuthGroup = p2panda_auth::group::GroupCrdt<
    p2panda_spaces::ActorId,
    p2panda_spaces::OperationId,
    p2panda_spaces::AuthMessage<()>,
    (),
    p2panda_spaces::StrongRemoveResolver<()>,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationRole {
    Pending,
    Known(Option<Role>),
}

impl BoardHandle {
    /// Read the author's role at the authorization heads signed by an application operation.
    pub async fn operation_role(&self, operation: EventId) -> Result<Option<Role>, Error> {
        Ok(match self.operation_role_state(operation).await? {
            OperationRole::Pending => None,
            OperationRole::Known(role) => role,
        })
    }

    pub async fn operation_role_state(&self, operation: EventId) -> Result<OperationRole, Error> {
        let store = SqliteStore::from_pool(self.membership_pool.clone());
        let id = Hash::from_bytes(*operation.as_bytes());
        let Some(op) = <SqliteStore as OperationStore<Operation, Hash>>::get_operation(&store, &id)
            .await
            .map_err(Error::storage)?
        else {
            return Ok(OperationRole::Pending);
        };
        let ExtensionsVariantV1::Space(extension) = &op.header.extensions.variant else {
            return Ok(OperationRole::Known(None));
        };
        let SpacesHeaderArgs::Application {
            space_id, proof, ..
        } = &extension.args
        else {
            return Ok(OperationRole::Known(None));
        };
        if space_id.as_bytes() != self.id().as_bytes() {
            return Ok(OperationRole::Known(None));
        }
        let spaces = SqliteSpacesStore::<Extensions>::new(store);
        let Some(state): Option<SpacesStoreState<()>> = spaces
            .get_space_state(space_id)
            .await
            .map_err(Error::storage)?
        else {
            return Ok(OperationRole::Pending);
        };
        let members = AuthGroup::members_at(
            &state.groups_y,
            proof.iter().copied().collect(),
            state.group_id,
        )
        .map_err(Error::storage)?;
        Ok(OperationRole::Known(members.into_iter().find_map(
            |(member, access)| {
                (member.as_bytes() == op.header.author().as_bytes())
                    .then(|| Role::from_level(access.level))
            },
        )))
    }

    /// A reconcile invitation must have observed every transport removal of the device.
    pub async fn permits_reenrollment(
        &self,
        device: DeviceId,
        invitation: Option<EventId>,
    ) -> Result<bool, Error> {
        use p2panda_auth::group::{GroupAction, GroupMember};

        let store = SqliteStore::from_pool(self.membership_pool.clone());
        let spaces = SqliteSpacesStore::<Extensions>::new(store.clone());
        let space = Hash::from_bytes(*self.id().as_bytes());
        let Some(state): Option<SpacesStoreState<()>> = spaces
            .get_space_state(&space)
            .await
            .map_err(Error::storage)?
        else {
            return Ok(false);
        };
        let device = MemberId::from_bytes(device.as_bytes()).map_err(Error::storage)?;
        if !state.removed.contains(&device) {
            return Ok(true);
        }
        let Some(invitation) = invitation else {
            return Ok(false);
        };
        let id = Hash::from_bytes(*invitation.as_bytes());
        let Some(op) = <SqliteStore as OperationStore<Operation, Hash>>::get_operation(&store, &id)
            .await
            .map_err(Error::storage)?
        else {
            return Ok(false);
        };
        let ExtensionsVariantV1::Space(extension) = &op.header.extensions.variant else {
            return Ok(false);
        };
        let SpacesHeaderArgs::Application {
            space_id, proof, ..
        } = &extension.args
        else {
            return Ok(false);
        };
        if *space_id != space {
            return Ok(false);
        }
        let operations = &state.groups_y.inner.operations;
        let mut pending = proof.to_vec();
        let mut covered = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if covered.insert(id)
                && let Some(op) = operations.get(&id)
            {
                pending.extend(op.dependencies.iter().copied());
            }
        }
        Ok(operations.values().filter(|op| {
            op.group_id == state.group_id
                && matches!(op.action, GroupAction::Remove { member: GroupMember::Individual(id) } if id == device)
        }).all(|op| covered.contains(&op.operation_id)))
    }

    /// Check whether a device was admitted by the named manager in this board's history.
    pub async fn admitted_by(&self, device: DeviceId, manager: DeviceId) -> Result<bool, Error> {
        use p2panda_auth::group::{GroupAction, GroupMember};
        let store = SqliteSpacesStore::<Extensions>::new(SqliteStore::from_pool(
            self.membership_pool.clone(),
        ));
        let state: Option<SpacesStoreState<()>> = store
            .get_space_state(&Hash::from_bytes(*self.id().as_bytes()))
            .await
            .map_err(Error::storage)?;
        let Some(state) = state else { return Ok(false) };
        for entry in state.groups_y.inner.operations.values() {
            if entry.group_id != state.group_id || entry.author.as_bytes() != manager.as_bytes() {
                continue;
            }
            let GroupAction::Add {
                member: GroupMember::Individual(member),
                access,
            } = &entry.action
            else {
                continue;
            };
            if member.as_bytes() != device.as_bytes() {
                continue;
            }
            let at = |heads| {
                AuthGroup::members_at(&state.groups_y, heads, state.group_id)
                    .map_err(Error::storage)
            };
            let before = at(entry.dependencies.iter().copied().collect())?;
            if before.iter().any(|(actor, _)| actor == member)
                || !before.iter().any(|(actor, access)| {
                    *actor == entry.author && access.level == AccessLevel::Manage
                })
            {
                continue;
            }
            let after = at([entry.operation_id].into())?;
            if after
                .iter()
                .any(|(actor, granted)| actor == member && granted.level == access.level)
            {
                return Ok(true);
            }
        }
        Ok(false)
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

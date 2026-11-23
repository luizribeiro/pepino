use pepino_core::{BoardId, DeviceId, IdentityId};
use sqlx::Connection;

use crate::{BoardKey, Store, StoreError};

impl Store {
    /// Checks the local binding, adopting an unbound legacy database only if it is
    /// empty or already contains events for the selected board.
    pub async fn bind_local(
        &self,
        board: BoardId,
        identity: IdentityId,
        device: DeviceId,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock().await;
        let binding = || {
            sqlx::query_as::<_, ([u8; 32], [u8; 32], [u8; 32])>(
                "SELECT board, identity, device FROM local_binding",
            )
        };
        let expected = (*board.as_bytes(), *identity.as_bytes(), *device.as_bytes());
        if let Some(saved) = binding().fetch_optional(&mut *connection).await? {
            return if saved == expected {
                Ok(())
            } else {
                Err(StoreError::LocalBindingMismatch)
            };
        }

        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(binding) = binding().fetch_optional(&mut *tx).await? {
            if binding != expected {
                return Err(StoreError::LocalBindingMismatch);
            }
        } else {
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM event_log WHERE board = ?)
                     OR NOT EXISTS (SELECT 1 FROM event_log)",
            )
            .bind(BoardKey(board))
            .fetch_one(&mut *tx)
            .await?;
            if !belongs {
                return Err(StoreError::LocalBindingMismatch);
            }
            sqlx::query("INSERT INTO local_binding VALUES (1, ?, ?, ?)")
                .bind(BoardKey(board))
                .bind(identity.as_bytes().as_slice())
                .bind(device.as_bytes().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_adopts_a_legacy_board_and_rejects_other_local_state() {
        let store = Store::open_in_memory().await.unwrap();
        let board = BoardId::from_bytes([21; 32]);
        let wrong = BoardId::from_bytes([34; 32]);
        let identity = IdentityId::from_bytes([41; 32]);
        let device = DeviceId::from_bytes([51; 32]);
        store
            .append(board, &[0x82, 0x02, 0xf6], None)
            .await
            .unwrap();
        assert!(matches!(
            store.bind_local(wrong, identity, device).await,
            Err(StoreError::LocalBindingMismatch)
        ));
        store.bind_local(board, identity, device).await.unwrap();
        store.bind_local(board, identity, device).await.unwrap();
        assert!(matches!(
            store
                .bind_local(board, IdentityId::from_bytes([61; 32]), device)
                .await,
            Err(StoreError::LocalBindingMismatch)
        ));
    }
}

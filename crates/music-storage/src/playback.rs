use music_application::playback::{
    PlaybackStateStore, StoreCompareAndSwap, StoreFuture,
    StoredPlaybackSnapshot as ApplicationSnapshot,
};

use crate::{CompareAndSwap, SqliteStorage, StorageError};

impl PlaybackStateStore for SqliteStorage {
    type Error = StorageError;

    fn load(&self, id: i64) -> StoreFuture<'_, Option<ApplicationSnapshot>, Self::Error> {
        Box::pin(async move {
            Ok(self
                .load_playback_snapshot(id)
                .await?
                .map(|snapshot| ApplicationSnapshot {
                    state_json: snapshot.state_json,
                    storage_revision: snapshot.storage_revision,
                }))
        })
    }

    fn insert_if_missing<'a>(
        &'a self,
        id: i64,
        state_json: &'a str,
    ) -> StoreFuture<'a, bool, Self::Error> {
        Box::pin(async move {
            self.insert_playback_snapshot_if_missing(id, state_json)
                .await
        })
    }

    fn compare_and_swap<'a>(
        &'a self,
        id: i64,
        expected_storage_revision: i64,
        state_json: &'a str,
    ) -> StoreFuture<'a, StoreCompareAndSwap, Self::Error> {
        Box::pin(async move {
            Ok(
                match self
                    .compare_and_swap_playback_snapshot(id, expected_storage_revision, state_json)
                    .await?
                {
                    CompareAndSwap::Updated { storage_revision } => {
                        StoreCompareAndSwap::Updated { storage_revision }
                    }
                    CompareAndSwap::Conflict => StoreCompareAndSwap::Conflict,
                },
            )
        })
    }
}

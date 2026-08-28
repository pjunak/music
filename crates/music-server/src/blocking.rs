use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use tokio::sync::Semaphore;

const MEDIA_WORKER_CAPACITY: usize = 2;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum BlockingMediaError {
    Busy,
    WorkerFailed,
}

impl Display for BlockingMediaError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "blocking media workers are busy",
            Self::WorkerFailed => "a blocking media worker failed",
        })
    }
}

impl Error for BlockingMediaError {}

/// Bounds blocking media work before it enters Tokio's blocking pool.
///
/// Admission deliberately uses `try_acquire_owned`: overload is rejected at
/// the request boundary instead of accumulating an unbounded semaphore wait
/// queue or blocking-pool queue.
#[derive(Debug, Clone)]
pub(crate) struct BlockingMediaExecutor {
    slots: Arc<Semaphore>,
}

impl Default for BlockingMediaExecutor {
    fn default() -> Self {
        Self::with_capacity(MEDIA_WORKER_CAPACITY)
    }
}

impl BlockingMediaExecutor {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(capacity)),
        }
    }

    pub(crate) async fn execute<T, F>(&self, work: F) -> Result<T, BlockingMediaError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| BlockingMediaError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| BlockingMediaError::WorkerFailed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    use super::{BlockingMediaError, BlockingMediaExecutor};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saturated_executor_rejects_instead_of_queueing()
    -> Result<(), Box<dyn std::error::Error>> {
        let executor = BlockingMediaExecutor::with_capacity(1);
        let occupied = executor.clone();
        let (started_tx, started_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let first = tokio::spawn(async move {
            occupied
                .execute(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    1_u8
                })
                .await
        });
        started_rx.recv_timeout(Duration::from_secs(1))?;

        assert_eq!(
            executor.execute(|| 2_u8).await,
            Err(BlockingMediaError::Busy)
        );
        release_tx.send(())?;
        assert_eq!(first.await??, 1);
        assert_eq!(executor.execute(|| 3_u8).await?, 3);
        Ok(())
    }
}

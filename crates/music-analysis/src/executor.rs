use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use tokio::sync::oneshot;

type Work = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    Run(Work),
    Shutdown,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AnalysisExecutorError {
    InvalidWorkerCount,
    SpawnFailed,
    Busy,
    Stopped,
}

impl Display for AnalysisExecutorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorkerCount => "analysis worker count must be between 1 and 4",
            Self::SpawnFailed => "analysis worker could not be started",
            Self::Busy => "analysis worker queue is full",
            Self::Stopped => "analysis executor has stopped",
        })
    }
}

impl Error for AnalysisExecutorError {}

struct ExecutorInner {
    sender: SyncSender<Message>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for ExecutorInner {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutorInner")
            .field(
                "workers",
                &self.workers.lock().map_or(0, |workers| workers.len()),
            )
            .finish_non_exhaustive()
    }
}

impl Drop for ExecutorInner {
    fn drop(&mut self) {
        let workers = match self.workers.get_mut() {
            Ok(workers) => workers,
            Err(poisoned) => poisoned.into_inner(),
        };
        for _ in 0..workers.len() {
            if self.sender.send(Message::Shutdown).is_err() {
                break;
            }
        }
        for worker in workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisExecutor {
    inner: Arc<ExecutorInner>,
}

impl AnalysisExecutor {
    pub fn new(worker_count: u8) -> Result<Self, AnalysisExecutorError> {
        if !(1..=4).contains(&worker_count) {
            return Err(AnalysisExecutorError::InvalidWorkerCount);
        }
        let capacity = usize::from(worker_count).saturating_mul(2);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::with_capacity(usize::from(worker_count));
        for index in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let worker = thread::Builder::new()
                .name(format!("music-analysis-{index}"))
                .spawn(move || worker_loop(&receiver))
                .map_err(|_| AnalysisExecutorError::SpawnFailed)?;
            workers.push(worker);
        }
        Ok(Self {
            inner: Arc::new(ExecutorInner {
                sender,
                workers: Mutex::new(workers),
            }),
        })
    }

    pub async fn execute<T, F>(&self, work: F) -> Result<T, AnalysisExecutorError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let task = Message::Run(Box::new(move || {
            let _ = sender.send(work());
        }));
        self.inner
            .sender
            .try_send(task)
            .map_err(|error| match error {
                TrySendError::Full(_) => AnalysisExecutorError::Busy,
                TrySendError::Disconnected(_) => AnalysisExecutorError::Stopped,
            })?;
        receiver.await.map_err(|_| AnalysisExecutorError::Stopped)
    }
}

fn worker_loop(receiver: &Arc<Mutex<Receiver<Message>>>) {
    loop {
        let message = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        match message {
            Ok(Message::Run(work)) => work(),
            Ok(Message::Shutdown) | Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisExecutor, AnalysisExecutorError};

    #[tokio::test]
    async fn executor_runs_work_on_its_named_fixed_pool() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            AnalysisExecutor::new(0).err(),
            Some(AnalysisExecutorError::InvalidWorkerCount)
        );
        let executor = AnalysisExecutor::new(2)?;
        let thread_name = executor
            .execute(|| {
                std::thread::current()
                    .name()
                    .unwrap_or("unnamed")
                    .to_owned()
            })
            .await?;
        assert!(thread_name.starts_with("music-analysis-"));
        Ok(())
    }
}

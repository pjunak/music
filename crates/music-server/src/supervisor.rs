use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::error::RuntimeError;
use crate::health::{ComponentStatus, HealthRegistry};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CriticalTaskError {
    pub code: &'static str,
}

impl CriticalTaskError {
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CriticalFailure {
    pub task: &'static str,
    pub code: &'static str,
}

#[derive(Debug)]
struct SupervisorInner {
    root: CancellationToken,
    tasks: TaskTracker,
    accepting: Mutex<bool>,
    failure: watch::Sender<Option<CriticalFailure>>,
    health: HealthRegistry,
}

#[derive(Debug, Clone)]
pub struct TaskSupervisor {
    inner: Arc<SupervisorInner>,
}

impl TaskSupervisor {
    #[must_use]
    pub fn new(health: HealthRegistry) -> Self {
        let (failure, _receiver) = watch::channel(None);
        Self {
            inner: Arc::new(SupervisorInner {
                root: CancellationToken::new(),
                tasks: TaskTracker::new(),
                accepting: Mutex::new(true),
                failure,
                health,
            }),
        }
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.root.clone()
    }

    #[must_use]
    pub fn failure(&self) -> Option<CriticalFailure> {
        *self.inner.failure.borrow()
    }

    pub fn spawn_critical<F>(
        &self,
        task: &'static str,
        component: &'static str,
        future: F,
    ) -> Result<(), RuntimeError>
    where
        F: Future<Output = Result<(), CriticalTaskError>> + Send + 'static,
    {
        let accepting = self
            .inner
            .accepting
            .lock()
            .map_err(|_| RuntimeError::SupervisorPoisoned)?;
        if !*accepting {
            return Err(RuntimeError::TaskAdmissionClosed);
        }

        let inner = Arc::clone(&self.inner);
        self.inner.tasks.spawn(async move {
            let result = tokio::spawn(future).await;
            match result {
                Ok(Ok(())) if inner.root.is_cancelled() => {}
                Ok(Ok(())) => {
                    record_failure(
                        &inner,
                        component,
                        CriticalFailure {
                            task,
                            code: "critical_task_exited",
                        },
                    );
                }
                Ok(Err(error)) => {
                    record_failure(
                        &inner,
                        component,
                        CriticalFailure {
                            task,
                            code: error.code,
                        },
                    );
                }
                Err(_) => {
                    record_failure(
                        &inner,
                        component,
                        CriticalFailure {
                            task,
                            code: "critical_task_panicked",
                        },
                    );
                }
            }
        });
        drop(accepting);
        Ok(())
    }

    pub async fn shutdown(&self, timeout: Duration) -> Result<(), RuntimeError> {
        {
            let mut accepting = self
                .inner
                .accepting
                .lock()
                .map_err(|_| RuntimeError::SupervisorPoisoned)?;
            *accepting = false;
            self.inner.tasks.close();
        }
        self.inner.root.cancel();
        tokio::time::timeout(timeout, self.inner.tasks.wait())
            .await
            .map_err(|_| RuntimeError::ShutdownTimedOut { timeout })?;
        Ok(())
    }
}

fn record_failure(inner: &SupervisorInner, component: &'static str, failure: CriticalFailure) {
    inner
        .health
        .set_component(component, true, ComponentStatus::Failed);
    inner.failure.send_modify(|current| {
        if current.is_none() {
            *current = Some(failure);
        }
    });
    tracing::error!(
        task = failure.task,
        failure_code = failure.code,
        "critical runtime task failed"
    );
    inner.root.cancel();
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use super::{CriticalTaskError, TaskSupervisor};
    use crate::error::RuntimeError;
    use crate::health::{ComponentStatus, HealthRegistry, ReadinessStatus};

    #[tokio::test]
    async fn cooperative_tasks_are_tracked_through_shutdown() -> Result<(), Box<dyn Error>> {
        let health = HealthRegistry::new();
        health.set_component("database", true, ComponentStatus::Ready);
        let supervisor = TaskSupervisor::new(health);
        let cancellation = supervisor.cancellation_token();
        supervisor.spawn_critical("database-monitor", "database", async move {
            cancellation.cancelled().await;
            Ok(())
        })?;

        supervisor.shutdown(Duration::from_secs(1)).await?;

        assert!(supervisor.cancellation_token().is_cancelled());
        assert!(supervisor.failure().is_none());
        assert!(matches!(
            supervisor.spawn_critical("late", "database", async { Ok(()) }),
            Err(RuntimeError::TaskAdmissionClosed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn task_errors_cancel_the_runtime_and_fail_readiness() -> Result<(), Box<dyn Error>> {
        let health = HealthRegistry::new();
        health.set_component("playback", true, ComponentStatus::Ready);
        let supervisor = TaskSupervisor::new(health.clone());
        supervisor.spawn_critical("playback-owner", "playback", async {
            Err(CriticalTaskError::new("playback_store_failed"))
        })?;

        tokio::time::timeout(
            Duration::from_secs(1),
            supervisor.cancellation_token().cancelled_owned(),
        )
        .await?;

        assert_eq!(health.snapshot().status, ReadinessStatus::NotReady);
        assert_eq!(
            supervisor.failure().map(|failure| failure.code),
            Some("playback_store_failed")
        );
        supervisor.shutdown(Duration::from_secs(1)).await?;
        Ok(())
    }
}

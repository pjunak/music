use std::collections::BTreeMap;

use serde::Serialize;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Starting,
    Ready,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Starting,
    Ready,
    Degraded,
    NotReady,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ReadinessSnapshot {
    pub status: ReadinessStatus,
    pub components: BTreeMap<&'static str, ComponentStatus>,
}

impl ReadinessSnapshot {
    #[must_use]
    pub fn accepts_traffic(&self) -> bool {
        matches!(
            self.status,
            ReadinessStatus::Ready | ReadinessStatus::Degraded
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ComponentRecord {
    critical: bool,
    status: ComponentStatus,
}

#[derive(Debug, Clone, Default)]
struct HealthState {
    components: BTreeMap<&'static str, ComponentRecord>,
}

#[derive(Debug, Clone)]
pub struct HealthRegistry {
    state: watch::Sender<HealthState>,
}

impl HealthRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (state, _receiver) = watch::channel(HealthState::default());
        Self { state }
    }

    pub fn set_component(&self, name: &'static str, critical: bool, status: ComponentStatus) {
        self.state.send_modify(|state| {
            state
                .components
                .insert(name, ComponentRecord { critical, status });
        });
    }

    #[must_use]
    pub fn snapshot(&self) -> ReadinessSnapshot {
        let state = self.state.borrow();
        let components = state
            .components
            .iter()
            .map(|(&name, record)| (name, record.status))
            .collect();
        let mut has_critical = false;
        let mut critical_starting = false;
        let mut critical_failed = false;
        let mut optional_degraded = false;
        for record in state.components.values() {
            if record.critical {
                has_critical = true;
                match record.status {
                    ComponentStatus::Ready => {}
                    ComponentStatus::Starting => critical_starting = true,
                    ComponentStatus::Degraded | ComponentStatus::Failed => critical_failed = true,
                }
            } else if matches!(
                record.status,
                ComponentStatus::Degraded | ComponentStatus::Failed
            ) {
                optional_degraded = true;
            }
        }
        let status = if critical_failed {
            ReadinessStatus::NotReady
        } else if !has_critical || critical_starting {
            ReadinessStatus::Starting
        } else if optional_degraded {
            ReadinessStatus::Degraded
        } else {
            ReadinessStatus::Ready
        };
        ReadinessSnapshot { status, components }
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentStatus, HealthRegistry, ReadinessStatus};

    #[test]
    fn critical_components_gate_readiness_and_optional_failures_degrade() {
        let health = HealthRegistry::new();
        assert_eq!(health.snapshot().status, ReadinessStatus::Starting);

        health.set_component("database", true, ComponentStatus::Starting);
        health.set_component("ffmpeg", false, ComponentStatus::Failed);
        assert_eq!(health.snapshot().status, ReadinessStatus::Starting);

        health.set_component("database", true, ComponentStatus::Ready);
        assert_eq!(health.snapshot().status, ReadinessStatus::Degraded);
        assert!(health.snapshot().accepts_traffic());

        health.set_component("playback", true, ComponentStatus::Failed);
        assert_eq!(health.snapshot().status, ReadinessStatus::NotReady);
        assert!(!health.snapshot().accepts_traffic());
    }
}

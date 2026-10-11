use std::collections::BTreeMap;

use serde::Serialize;
use tokio::sync::watch;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Starting,
    Ready,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Starting,
    Ready,
    Degraded,
    NotReady,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
pub struct ReadinessSnapshot {
    pub status: ReadinessStatus,
    pub components: BTreeMap<String, ComponentStatus>,
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

/// A runtime owner reported by readiness. Critical components gate traffic;
/// optional ones only degrade it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum Component {
    Authentication,
    BackgroundJobs,
    Configuration,
    Database,
    DatabaseSchema,
    Ffmpeg,
    Filesystem,
    Http,
    InstanceLock,
    Library,
    LibraryCoordinator,
    ModeCoordinator,
    ModeSeed,
    Modes,
    Playback,
    Runtime,
    Sfx,
    StaticFiles,
    VoiceAnalysis,
}

impl Component {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::BackgroundJobs => "background_jobs",
            Self::Configuration => "configuration",
            Self::Database => "database",
            Self::DatabaseSchema => "database_schema",
            Self::Ffmpeg => "ffmpeg",
            Self::Filesystem => "filesystem",
            Self::Http => "http",
            Self::InstanceLock => "instance_lock",
            Self::Library => "library",
            Self::LibraryCoordinator => "library_coordinator",
            Self::ModeCoordinator => "mode_coordinator",
            Self::ModeSeed => "mode_seed",
            Self::Modes => "modes",
            Self::Playback => "playback",
            Self::Runtime => "runtime",
            Self::Sfx => "sfx",
            Self::StaticFiles => "static_files",
            Self::VoiceAnalysis => "voice_analysis",
        }
    }

    #[must_use]
    pub const fn critical(self) -> bool {
        !matches!(
            self,
            Self::Ffmpeg
                | Self::Library
                | Self::ModeSeed
                | Self::Modes
                | Self::StaticFiles
                | Self::VoiceAnalysis
        )
    }
}

#[derive(Debug, Clone, Default)]
struct HealthState {
    components: BTreeMap<Component, ComponentStatus>,
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

    pub fn set_component(&self, component: Component, status: ComponentStatus) {
        self.state.send_modify(|state| {
            state.components.insert(component, status);
        });
    }

    #[must_use]
    pub fn snapshot(&self) -> ReadinessSnapshot {
        let state = self.state.borrow();
        let components = state
            .components
            .iter()
            .map(|(component, status)| (component.name().to_owned(), *status))
            .collect();
        let mut has_critical = false;
        let mut critical_starting = false;
        let mut critical_failed = false;
        let mut optional_degraded = false;
        for (component, status) in &state.components {
            if component.critical() {
                has_critical = true;
                match status {
                    ComponentStatus::Ready => {}
                    ComponentStatus::Starting => critical_starting = true,
                    ComponentStatus::Degraded | ComponentStatus::Failed => critical_failed = true,
                }
            } else if matches!(status, ComponentStatus::Degraded | ComponentStatus::Failed) {
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
    use super::{Component, ComponentStatus, HealthRegistry, ReadinessStatus};

    #[test]
    fn critical_components_gate_readiness_and_optional_failures_degrade() {
        let health = HealthRegistry::new();
        assert_eq!(health.snapshot().status, ReadinessStatus::Starting);

        health.set_component(Component::Database, ComponentStatus::Starting);
        health.set_component(Component::Ffmpeg, ComponentStatus::Failed);
        assert_eq!(health.snapshot().status, ReadinessStatus::Starting);

        health.set_component(Component::Database, ComponentStatus::Ready);
        assert_eq!(health.snapshot().status, ReadinessStatus::Degraded);
        assert!(health.snapshot().accepts_traffic());

        health.set_component(Component::Playback, ComponentStatus::Failed);
        assert_eq!(health.snapshot().status, ReadinessStatus::NotReady);
        assert!(!health.snapshot().accepts_traffic());
    }
}

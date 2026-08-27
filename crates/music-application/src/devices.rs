use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type DeviceDependencyError = Box<dyn Error + Send + Sync + 'static>;
pub type DeviceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DeviceDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RememberedDevice {
    pub client_id: String,
    pub name: String,
    pub is_output: bool,
    pub added_at: Option<String>,
}

pub trait RememberedDeviceRepository: Send + Sync + 'static {
    fn list_devices(&self) -> DeviceFuture<'_, Vec<RememberedDevice>>;

    fn find_device<'a>(&'a self, client_id: &'a str) -> DeviceFuture<'a, Option<RememberedDevice>>;

    fn upsert_device<'a>(
        &'a self,
        client_id: &'a str,
        name: &'a str,
        is_output: bool,
    ) -> DeviceFuture<'a, RememberedDevice>;

    fn delete_device<'a>(&'a self, client_id: &'a str) -> DeviceFuture<'a, bool>;
}

#[derive(Debug)]
pub struct RememberedDeviceService<R> {
    repository: Arc<R>,
}

impl<R> Clone for RememberedDeviceService<R> {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
        }
    }
}

impl<R> RememberedDeviceService<R>
where
    R: RememberedDeviceRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn list(&self) -> Result<Vec<RememberedDevice>, DeviceServiceError> {
        self.repository
            .list_devices()
            .await
            .map_err(|source| DeviceServiceError::dependency("device listing", source))
    }

    pub async fn find(
        &self,
        client_id: &str,
    ) -> Result<Option<RememberedDevice>, DeviceServiceError> {
        self.repository
            .find_device(client_id)
            .await
            .map_err(|source| DeviceServiceError::dependency("device lookup", source))
    }

    pub async fn is_default_output(&self, client_id: &str) -> Result<bool, DeviceServiceError> {
        Ok(self
            .find(client_id)
            .await?
            .is_some_and(|device| device.is_output))
    }

    pub async fn save(
        &self,
        client_id: &str,
        name: &str,
        is_output: bool,
    ) -> Result<RememberedDevice, DeviceServiceError> {
        validate_client_id(client_id)?;
        if !(1..=128).contains(&name.chars().count()) {
            return Err(DeviceServiceError::InvalidName);
        }
        self.repository
            .upsert_device(client_id, name, is_output)
            .await
            .map_err(|source| DeviceServiceError::dependency("device save", source))
    }

    pub async fn forget(&self, client_id: &str) -> Result<bool, DeviceServiceError> {
        validate_client_id(client_id)?;
        self.repository
            .delete_device(client_id)
            .await
            .map_err(|source| DeviceServiceError::dependency("device deletion", source))
    }
}

fn validate_client_id(client_id: &str) -> Result<(), DeviceServiceError> {
    if !(1..=256).contains(&client_id.chars().count()) || client_id.chars().any(char::is_control) {
        Err(DeviceServiceError::InvalidClientId)
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum DeviceServiceError {
    InvalidClientId,
    InvalidName,
    Dependency {
        operation: &'static str,
        source: DeviceDependencyError,
    },
}

impl DeviceServiceError {
    fn dependency(operation: &'static str, source: DeviceDependencyError) -> Self {
        Self::Dependency { operation, source }
    }
}

impl Display for DeviceServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClientId => formatter.write_str("invalid device client ID"),
            Self::InvalidName => formatter.write_str("invalid device name"),
            Self::Dependency { operation, .. } => {
                write!(
                    formatter,
                    "remembered-device dependency failed during {operation}"
                )
            }
        }
    }
}

impl Error for DeviceServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dependency { source, .. } => Some(source.as_ref()),
            Self::InvalidClientId | Self::InvalidName => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::{
        DeviceFuture, RememberedDevice, RememberedDeviceRepository, RememberedDeviceService,
    };

    #[derive(Debug, Default)]
    struct MemoryRepository(Mutex<BTreeMap<String, RememberedDevice>>);

    impl RememberedDeviceRepository for MemoryRepository {
        fn list_devices(&self) -> DeviceFuture<'_, Vec<RememberedDevice>> {
            Box::pin(async move {
                let guard = match self.0.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                Ok(guard.values().cloned().collect())
            })
        }

        fn find_device<'a>(
            &'a self,
            client_id: &'a str,
        ) -> DeviceFuture<'a, Option<RememberedDevice>> {
            Box::pin(async move {
                let guard = match self.0.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                Ok(guard.get(client_id).cloned())
            })
        }

        fn upsert_device<'a>(
            &'a self,
            client_id: &'a str,
            name: &'a str,
            is_output: bool,
        ) -> DeviceFuture<'a, RememberedDevice> {
            Box::pin(async move {
                let device = RememberedDevice {
                    client_id: client_id.to_owned(),
                    name: name.to_owned(),
                    is_output,
                    added_at: None,
                };
                let mut guard = match self.0.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.insert(client_id.to_owned(), device.clone());
                Ok(device)
            })
        }

        fn delete_device<'a>(&'a self, client_id: &'a str) -> DeviceFuture<'a, bool> {
            Box::pin(async move {
                let mut guard = match self.0.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                Ok(guard.remove(client_id).is_some())
            })
        }
    }

    #[tokio::test]
    async fn designation_is_explicit_and_independent_from_connection_state() {
        let service =
            RememberedDeviceService::new(std::sync::Arc::new(MemoryRepository::default()));
        assert!(
            !service
                .is_default_output("living-room")
                .await
                .unwrap_or(true)
        );
        let saved = service.save("living-room", "Living room", true).await;
        assert!(saved.is_ok());
        assert!(
            service
                .is_default_output("living-room")
                .await
                .unwrap_or(false)
        );
        assert!(service.forget("living-room").await.unwrap_or(false));
        assert!(
            !service
                .is_default_output("living-room")
                .await
                .unwrap_or(true)
        );
    }
}

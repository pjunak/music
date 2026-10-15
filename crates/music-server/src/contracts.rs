use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::http::openapi_document;

mod typescript_http;

const TYPESCRIPT_PATH: &str = "frontend/src/generated/protocol.ts";
const HTTP_TYPESCRIPT_PATH: &str = "frontend/src/generated/http.ts";
const OPENAPI_PATH: &str = "contracts/generated/rust/openapi.json";

#[derive(Debug)]
pub enum ContractError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Json {
        document: &'static str,
        source: serde_json::Error,
    },
    InvalidArtifactPath(&'static str),
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::Json { document, .. } => {
                write!(formatter, "failed to render the {document} contract")
            }
            Self::InvalidArtifactPath(path) => {
                write!(formatter, "generated contract path has no parent: {path}")
            }
        }
    }
}

impl Error for ContractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidArtifactPath(_) => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContractArtifact {
    pub relative_path: &'static str,
    pub content: String,
}

/// Render every checked-in contract from the authoritative Rust declarations.
pub fn render_contracts() -> Result<Vec<ContractArtifact>, ContractError> {
    // Going through `Value` sorts object keys, keeping the files stable.
    let openapi =
        serde_json::to_value(openapi_document()).map_err(|source| ContractError::Json {
            document: "OpenAPI",
            source,
        })?;
    Ok(vec![
        ContractArtifact {
            relative_path: TYPESCRIPT_PATH,
            content: music_protocol::typescript_bindings(),
        },
        ContractArtifact {
            relative_path: HTTP_TYPESCRIPT_PATH,
            content: typescript_http::http_typescript(&openapi),
        },
        ContractArtifact {
            relative_path: OPENAPI_PATH,
            content: pretty_json(&openapi, "OpenAPI")?,
        },
    ])
}

/// Write only changed generated artifacts and return their relative paths.
pub fn export_contracts(repository_root: &Path) -> Result<Vec<PathBuf>, ContractError> {
    let mut changed = Vec::new();
    for artifact in render_contracts()? {
        let path = repository_root.join(artifact.relative_path);
        if read_optional(&path)?.as_deref() == Some(artifact.content.as_str()) {
            continue;
        }
        let parent = path
            .parent()
            .ok_or(ContractError::InvalidArtifactPath(artifact.relative_path))?;
        fs::create_dir_all(parent).map_err(|source| ContractError::Io {
            operation: "create contract directory",
            path: parent.to_owned(),
            source,
        })?;
        fs::write(&path, artifact.content.as_bytes()).map_err(|source| ContractError::Io {
            operation: "write generated contract",
            path: path.clone(),
            source,
        })?;
        changed.push(PathBuf::from(artifact.relative_path));
    }
    Ok(changed)
}

/// Return generated files that are missing or differ from authoritative Rust.
pub fn check_contracts(repository_root: &Path) -> Result<Vec<PathBuf>, ContractError> {
    let mut drifted = Vec::new();
    for artifact in render_contracts()? {
        let path = repository_root.join(artifact.relative_path);
        if read_optional(&path)?.as_deref() != Some(artifact.content.as_str()) {
            drifted.push(PathBuf::from(artifact.relative_path));
        }
    }
    Ok(drifted)
}

fn read_optional(path: &Path) -> Result<Option<String>, ContractError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ContractError::Io {
            operation: "read generated contract",
            path: path.to_owned(),
            source,
        }),
    }
}

fn pretty_json<T: Serialize>(value: &T, document: &'static str) -> Result<String, ContractError> {
    let mut rendered = serde_json::to_string_pretty(value)
        .map_err(|source| ContractError::Json { document, source })?;
    rendered.push('\n');
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;

    use super::openapi_document;

    #[test]
    fn route_registration_is_the_openapi_source_of_truth() -> Result<(), Box<dyn Error>> {
        let document = serde_json::to_value(openapi_document())?;
        let paths = document["paths"]
            .as_object()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "paths is not an object"))?;
        assert!(paths.contains_key("/api/health"));
        assert!(paths.contains_key("/api/readiness"));
        assert!(paths.contains_key("/api/sync/state"));
        assert!(paths.contains_key("/api/auth/login"));
        assert!(paths.contains_key("/api/auth/logout"));
        assert!(paths.contains_key("/api/auth/me"));
        assert!(paths.contains_key("/api/auth/sessions"));
        assert!(paths.contains_key("/api/auth/sessions/{session_id}"));
        assert!(paths.contains_key("/api/devices"));
        assert!(paths.contains_key("/api/devices/{client_id}"));
        assert!(paths.contains_key("/api/library/search"));
        assert!(paths.contains_key("/api/library/tree"));
        let folders = paths["/api/library/folders"].as_object().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "folder path is not an object")
        })?;
        assert!(folders.contains_key("get"));
        assert!(folders.contains_key("post"));
        assert!(folders.contains_key("delete"));
        assert!(paths.contains_key("/api/library/folders/rename"));
        assert!(paths.contains_key("/api/library/tracks"));
        assert!(paths.contains_key("/api/library/tracks/bulk-metadata"));
        assert!(paths.contains_key("/api/library/tracks/bulk-move"));
        assert!(paths.contains_key("/api/library/tracks/bulk-delete"));
        let track = paths["/api/library/tracks/{track_id}"]
            .as_object()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "track path is not an object")
            })?;
        assert!(track.contains_key("get"));
        assert!(track.contains_key("delete"));
        assert!(paths.contains_key("/api/library/tracks/{track_id}/move"));
        assert!(paths.contains_key("/api/library/tracks/{track_id}/metadata"));
        assert!(paths.contains_key("/api/library/tracks/{track_id}/stream"));
        assert!(paths.contains_key("/api/library/tracks/{track_id}/cover"));
        assert!(paths.contains_key("/api/library/upload"));
        assert!(paths.contains_key("/api/library/upload/check"));
        assert!(paths.contains_key("/api/library/cleanup/analyze"));
        assert!(paths.contains_key("/api/library/cleanup/enrichment-jobs"));
        assert!(paths.contains_key("/api/library/cleanup/verify"));
        assert!(paths.contains_key("/api/library/rescan"));
        assert!(paths.contains_key("/api/modes"));
        assert!(paths.contains_key("/api/modes/active"));
        assert!(paths.contains_key("/api/modes/reload"));
        assert!(paths.contains_key("/api/modes/{mode_id}"));
        assert!(paths.contains_key("/api/modes/{mode_id}/presets"));
        assert!(paths.contains_key("/api/modes/{mode_id}/theme.css"));
        assert!(!paths.contains_key("/health"));
        assert!(!paths.contains_key("/api/ws"));
        Ok(())
    }

    /// Operation IDs default to the handler's function name, so two modules
    /// with a same-named handler would collide silently.
    #[test]
    fn operation_ids_are_unique() -> Result<(), Box<dyn Error>> {
        let document = serde_json::to_value(openapi_document())?;
        let paths = document["paths"]
            .as_object()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "paths is not an object"))?;
        let mut seen = std::collections::BTreeSet::new();
        for operation in paths.values().filter_map(serde_json::Value::as_object) {
            for id in operation
                .values()
                .filter_map(|operation| operation["operationId"].as_str())
            {
                assert!(seen.insert(id.to_owned()), "duplicate operationId {id}");
            }
        }
        assert!(seen.len() > 100);
        Ok(())
    }

    /// OpenAPI 3.1 requires a response description, and Utoipa omits it
    /// instead of emitting an empty one when a route declares none.
    #[test]
    fn every_documented_response_has_a_description() -> Result<(), Box<dyn Error>> {
        let document = serde_json::to_value(openapi_document())?;
        let paths = document["paths"]
            .as_object()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "paths is not an object"))?;
        let mut missing = Vec::new();
        for (path, operations) in paths {
            let Some(operations) = operations.as_object() else {
                continue;
            };
            for (method, operation) in operations {
                let Some(responses) = operation["responses"].as_object() else {
                    continue;
                };
                for (status, response) in responses {
                    if response["description"].as_str().is_none_or(str::is_empty) {
                        missing.push(format!("{method} {path} {status}"));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "responses without a description: {missing:?}"
        );
        Ok(())
    }
}

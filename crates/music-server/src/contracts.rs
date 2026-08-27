use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::http::openapi_document;

const TYPESCRIPT_PATH: &str = "frontend/src/generated/protocol.ts";
const OPENAPI_PATH: &str = "contracts/generated/rust/openapi.json";
const COMPATIBILITY_PATH: &str = "contracts/generated/rust/openapi-compatibility.json";
const REFERENCE_OPENAPI_PATH: &str = "contracts/reference/v1/openapi.json";
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

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
    InvalidOpenApi {
        document: &'static str,
        detail: &'static str,
    },
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::Json { document, .. } => {
                write!(
                    formatter,
                    "failed to process the {document} OpenAPI document"
                )
            }
            Self::InvalidOpenApi { document, detail } => {
                write!(formatter, "invalid {document} OpenAPI document: {detail}")
            }
        }
    }
}

impl Error for ContractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidOpenApi { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContractArtifact {
    pub relative_path: &'static str,
    pub content: String,
}

/// Render every checked-in contract from the authoritative Rust declarations.
pub fn render_contracts(repository_root: &Path) -> Result<Vec<ContractArtifact>, ContractError> {
    let reference_path = repository_root.join(REFERENCE_OPENAPI_PATH);
    let reference_json = read_to_string(&reference_path, "read")?;
    let reference: Value =
        serde_json::from_str(&reference_json).map_err(|source| ContractError::Json {
            document: "reference",
            source,
        })?;

    let candidate =
        serde_json::to_value(openapi_document()).map_err(|source| ContractError::Json {
            document: "Rust",
            source,
        })?;
    let compatibility = compare_openapi(&reference, &candidate)?;

    Ok(vec![
        ContractArtifact {
            relative_path: TYPESCRIPT_PATH,
            content: music_protocol::typescript_bindings(),
        },
        ContractArtifact {
            relative_path: OPENAPI_PATH,
            content: pretty_json(&candidate, "Rust")?,
        },
        ContractArtifact {
            relative_path: COMPATIBILITY_PATH,
            content: pretty_json(&compatibility, "compatibility report")?,
        },
    ])
}

/// Write only changed generated artifacts and return their relative paths.
pub fn export_contracts(repository_root: &Path) -> Result<Vec<PathBuf>, ContractError> {
    let mut changed = Vec::new();
    for artifact in render_contracts(repository_root)? {
        let path = repository_root.join(artifact.relative_path);
        if read_optional(&path)?.as_deref() == Some(artifact.content.as_str()) {
            continue;
        }
        let parent = path.parent().ok_or(ContractError::InvalidOpenApi {
            document: "generated",
            detail: "artifact path has no parent directory",
        })?;
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
    for artifact in render_contracts(repository_root)? {
        let path = repository_root.join(artifact.relative_path);
        if read_optional(&path)?.as_deref() != Some(artifact.content.as_str()) {
            drifted.push(PathBuf::from(artifact.relative_path));
        }
    }
    Ok(drifted)
}

fn read_to_string(path: &Path, operation: &'static str) -> Result<String, ContractError> {
    fs::read_to_string(path).map_err(|source| ContractError::Io {
        operation,
        path: path.to_owned(),
        source,
    })
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

#[derive(Debug, Serialize)]
struct OpenApiCompatibilityReport {
    schema_version: &'static str,
    verdict: &'static str,
    reference_operation_count: usize,
    candidate_operation_count: usize,
    matched_operation_count: usize,
    fully_compatible_operation_count: usize,
    missing_operations: Vec<String>,
    candidate_only_operations: Vec<String>,
    parameter_mismatches: Vec<String>,
    request_body_mismatches: Vec<String>,
    response_mismatches: Vec<String>,
    security_mismatches: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct OperationContract {
    parameters: Value,
    request_body: Value,
    responses: Value,
    security: Value,
}

fn compare_openapi(
    reference: &Value,
    candidate: &Value,
) -> Result<OpenApiCompatibilityReport, ContractError> {
    let reference_operations = operation_contracts(reference, "reference")?;
    let candidate_operations = operation_contracts(candidate, "Rust")?;
    let reference_keys = reference_operations
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidate_keys = candidate_operations
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let missing_operations = reference_keys
        .difference(&candidate_keys)
        .cloned()
        .collect::<Vec<_>>();
    let candidate_only_operations = candidate_keys
        .difference(&reference_keys)
        .cloned()
        .collect::<Vec<_>>();
    let matched = reference_keys
        .intersection(&candidate_keys)
        .cloned()
        .collect::<Vec<_>>();

    let mut fully_compatible_operation_count = 0;
    let mut parameter_mismatches = Vec::new();
    let mut request_body_mismatches = Vec::new();
    let mut response_mismatches = Vec::new();
    let mut security_mismatches = Vec::new();
    for key in &matched {
        let Some(reference_operation) = reference_operations.get(key) else {
            continue;
        };
        let Some(candidate_operation) = candidate_operations.get(key) else {
            continue;
        };
        let parameters_match = reference_operation.parameters == candidate_operation.parameters;
        let request_body_matches =
            reference_operation.request_body == candidate_operation.request_body;
        let responses_match = reference_operation.responses == candidate_operation.responses;
        let security_matches = reference_operation.security == candidate_operation.security;
        if !parameters_match {
            parameter_mismatches.push(key.clone());
        }
        if !request_body_matches {
            request_body_mismatches.push(key.clone());
        }
        if !responses_match {
            response_mismatches.push(key.clone());
        }
        if !security_matches {
            security_mismatches.push(key.clone());
        }
        if parameters_match && request_body_matches && responses_match && security_matches {
            fully_compatible_operation_count += 1;
        }
    }

    let compatible = missing_operations.is_empty()
        && parameter_mismatches.is_empty()
        && request_body_mismatches.is_empty()
        && response_mismatches.is_empty()
        && security_mismatches.is_empty();
    Ok(OpenApiCompatibilityReport {
        schema_version: "openapi-compatibility/v1",
        verdict: if compatible {
            "compatible"
        } else {
            "incomplete"
        },
        reference_operation_count: reference_operations.len(),
        candidate_operation_count: candidate_operations.len(),
        matched_operation_count: matched.len(),
        fully_compatible_operation_count,
        missing_operations,
        candidate_only_operations,
        parameter_mismatches,
        request_body_mismatches,
        response_mismatches,
        security_mismatches,
    })
}

fn operation_contracts(
    document: &Value,
    document_name: &'static str,
) -> Result<BTreeMap<String, OperationContract>, ContractError> {
    let paths =
        document
            .get("paths")
            .and_then(Value::as_object)
            .ok_or(ContractError::InvalidOpenApi {
                document: document_name,
                detail: "paths must be an object",
            })?;
    let root_security = document
        .get("security")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let mut contracts = BTreeMap::new();
    for (path, path_item) in paths {
        let Some(path_object) = path_item.as_object() else {
            continue;
        };
        for method in HTTP_METHODS {
            let Some(operation) = path_object.get(method).and_then(Value::as_object) else {
                continue;
            };
            let parameters = merged_parameters(document, path_object, operation);
            let request_body = operation
                .get("requestBody")
                .map_or(Value::Null, |value| normalize(document, value, None));
            let responses = operation
                .get("responses")
                .map_or(Value::Null, |value| normalize(document, value, None));
            let security = operation.get("security").unwrap_or(&root_security);
            contracts.insert(
                format!("{} {path}", method.to_ascii_uppercase()),
                OperationContract {
                    parameters,
                    request_body,
                    responses,
                    security: normalize(document, security, Some("security")),
                },
            );
        }
    }
    Ok(contracts)
}

fn merged_parameters(
    document: &Value,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
) -> Value {
    let mut parameters = BTreeMap::<String, Value>::new();
    for source in [path_item.get("parameters"), operation.get("parameters")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        for parameter in source {
            let normalized = normalize(document, parameter, None);
            let name = normalized.get("name").and_then(Value::as_str).unwrap_or("");
            let location = normalized.get("in").and_then(Value::as_str).unwrap_or("");
            let key = if name.is_empty() && location.is_empty() {
                serde_json::to_string(&normalized).unwrap_or_default()
            } else {
                format!("{location}:{name}")
            };
            parameters.insert(key, normalized);
        }
    }
    Value::Array(parameters.into_values().collect())
}

fn normalize(document: &Value, value: &Value, parent_key: Option<&str>) -> Value {
    normalize_inner(document, value, parent_key, &mut BTreeSet::new())
}

fn normalize_inner(
    document: &Value,
    value: &Value,
    parent_key: Option<&str>,
    resolving: &mut BTreeSet<String>,
) -> Value {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(pointer) = reference.strip_prefix('#')
                && resolving.insert(reference.to_owned())
            {
                let resolved = document.pointer(pointer).map_or_else(
                    || value.clone(),
                    |target| normalize_inner(document, target, parent_key, resolving),
                );
                resolving.remove(reference);
                if object.len() == 1 {
                    return resolved;
                }
                let siblings = object
                    .iter()
                    .filter(|(key, _)| key.as_str() != "$ref")
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            normalize_inner(document, value, Some(key), resolving),
                        )
                    })
                    .collect::<Map<_, _>>();
                return Value::Object(Map::from_iter([(
                    "allOf".to_owned(),
                    Value::Array(vec![resolved, Value::Object(siblings)]),
                )]));
            }

            let normalized = object
                .iter()
                .filter(|(key, _)| !is_descriptive_key(key))
                .map(|(key, value)| {
                    (
                        key.clone(),
                        normalize_inner(document, value, Some(key), resolving),
                    )
                })
                .collect::<Map<_, _>>();
            Value::Object(normalized)
        }
        Value::Array(values) => {
            let mut normalized = values
                .iter()
                .map(|value| normalize_inner(document, value, None, resolving))
                .collect::<Vec<_>>();
            if parent_key.is_some_and(is_unordered_array) {
                normalized.sort_by_key(|value| serde_json::to_string(value).unwrap_or_default());
                normalized.dedup();
            }
            Value::Array(normalized)
        }
        _ => value.clone(),
    }
}

fn is_descriptive_key(key: &str) -> bool {
    matches!(
        key,
        "description" | "summary" | "title" | "operationId" | "tags" | "example" | "examples"
    )
}

fn is_unordered_array(key: &str) -> bool {
    matches!(
        key,
        "required" | "enum" | "allOf" | "anyOf" | "oneOf" | "security"
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;

    use serde_json::json;

    use super::{compare_openapi, openapi_document};

    #[test]
    fn route_registration_is_the_openapi_source_of_truth() -> Result<(), Box<dyn Error>> {
        let document = serde_json::to_value(openapi_document())?;
        let paths = document["paths"]
            .as_object()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "paths is not an object"))?;
        assert!(paths.contains_key("/api/health"));
        assert!(paths.contains_key("/api/readiness"));
        assert!(!paths.contains_key("/health"));
        assert!(!paths.contains_key("/api/ws"));
        Ok(())
    }

    #[test]
    fn semantic_comparison_ignores_docs_and_resolves_local_schema_refs()
    -> Result<(), Box<dyn Error>> {
        let reference = json!({
            "openapi": "3.1.0",
            "paths": {"/thing": {"get": {
                "summary": "old words",
                "responses": {"200": {"description": "ok", "content": {
                    "application/json": {"schema": {"$ref": "#/components/schemas/Thing"}}
                }}}
            }}},
            "components": {"schemas": {"Thing": {
                "type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]
            }}}
        });
        let candidate = json!({
            "openapi": "3.1.0",
            "paths": {"/thing": {"get": {
                "summary": "new words",
                "responses": {"200": {"description": "different", "content": {
                    "application/json": {"schema": {
                        "title": "Thing", "required": ["id"], "properties": {"id": {"type": "integer"}}, "type": "object"
                    }}
                }}}
            }}}
        });
        let report = compare_openapi(&reference, &candidate)?;
        assert_eq!(report.verdict, "compatible");
        assert_eq!(report.fully_compatible_operation_count, 1);
        Ok(())
    }
}

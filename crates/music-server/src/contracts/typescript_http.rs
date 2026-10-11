//! TypeScript declarations for the HTTP API, rendered from the generated
//! OpenAPI document so the frontend compiles against the server's DTOs.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Map, Value};

const HEADER: &str = "// Generated from the Rust OpenAPI document by `music-cli contracts export`.\n// Do not edit by hand.\n";

/// Render one exported type per OpenAPI component schema.
///
/// Schemas reachable from a response treat nullable properties as always
/// present: the server serializes `None` as `null` rather than omitting it.
/// Request-only schemas keep them optional because missing values are
/// accepted on input.
#[must_use]
pub(super) fn http_typescript(document: &Value) -> String {
    let mut output = String::from(HEADER);
    let schemas = document
        .pointer("/components/schemas")
        .and_then(Value::as_object);
    let responses = response_schemas(document);
    for (name, schema) in schemas.into_iter().flatten() {
        let response = responses.contains(name.as_str());
        output.push('\n');
        if let Some(description) = schema.get("description").and_then(Value::as_str) {
            push_doc_comment(&mut output, description, "");
        }
        let _ = writeln!(
            output,
            "export type {} = {};",
            identifier(name),
            render(schema, 0, response)
        );
    }
    output
}

/// Names of component schemas reachable from any operation's responses.
fn response_schemas(document: &Value) -> BTreeSet<&str> {
    let schemas = document.pointer("/components/schemas");
    let mut pending = Vec::new();
    for operations in document
        .get("paths")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
    {
        for operation in operations.as_object().into_iter().flat_map(Map::values) {
            if let Some(responses) = operation.get("responses") {
                collect_references(responses, &mut pending);
            }
        }
    }
    let mut reachable = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if reachable.insert(name)
            && let Some(schema) = schemas.and_then(|schemas| schemas.get(name))
        {
            collect_references(schema, &mut pending);
        }
    }
    reachable
}

fn collect_references<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/components/schemas/"))
            {
                found.push(name);
            }
            map.values()
                .for_each(|nested| collect_references(nested, found));
        }
        Value::Array(items) => items
            .iter()
            .for_each(|nested| collect_references(nested, found)),
        _ => {}
    }
}

fn render(schema: &Value, depth: usize, response: bool) -> String {
    let Some(schema) = schema.as_object() else {
        return if schema.as_bool() == Some(false) {
            "never".to_owned()
        } else {
            "unknown".to_owned()
        };
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .map_or_else(|| "unknown".to_owned(), identifier);
    }
    if let Some(value) = schema.get("const") {
        return literal(value);
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return union(values.iter().map(literal));
    }
    for (keyword, separator) in [("anyOf", " | "), ("oneOf", " | "), ("allOf", " & ")] {
        if let Some(variants) = schema.get(keyword).and_then(Value::as_array) {
            let parts = variants
                .iter()
                .map(|variant| parenthesize(render(variant, depth, response)))
                .collect::<Vec<_>>();
            return dedup_join(parts, separator);
        }
    }
    match schema.get("type") {
        Some(Value::String(kind)) => render_type(kind, schema, depth, response),
        Some(Value::Array(kinds)) => union(
            kinds
                .iter()
                .filter_map(Value::as_str)
                .map(|kind| parenthesize(render_type(kind, schema, depth, response))),
        ),
        _ if schema.contains_key("properties") => render_object(schema, depth, response),
        _ => "unknown".to_owned(),
    }
}

fn render_type(kind: &str, schema: &Map<String, Value>, depth: usize, response: bool) -> String {
    match kind {
        "string" => "string".to_owned(),
        "integer" | "number" => "number".to_owned(),
        "boolean" => "boolean".to_owned(),
        "null" => "null".to_owned(),
        "array" => match schema.get("items") {
            Some(items) => format!("{}[]", parenthesize(render(items, depth, response))),
            None => "unknown[]".to_owned(),
        },
        "object" => render_object(schema, depth, response),
        _ => "unknown".to_owned(),
    }
}

fn render_object(schema: &Map<String, Value>, depth: usize, response: bool) -> String {
    let properties = schema.get("properties").and_then(Value::as_object);
    let additional = schema.get("additionalProperties");
    let index = match additional {
        Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some("unknown".to_owned()),
        Some(Value::Object(value)) => {
            Some(render(&Value::Object(value.clone()), depth + 1, response))
        }
        _ if properties.is_none() => Some("unknown".to_owned()),
        _ => None,
    };
    let Some(properties) = properties.filter(|properties| !properties.is_empty()) else {
        return format!(
            "Record<string, {}>",
            index.unwrap_or_else(|| "never".to_owned())
        );
    };
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let indent = "  ".repeat(depth + 1);
    let mut output = String::from("{\n");
    for (name, property) in properties {
        if let Some(description) = property.get("description").and_then(Value::as_str) {
            push_doc_comment(&mut output, description, &indent);
        }
        let present = required.contains(&name.as_str()) || (response && is_nullable(property));
        let optional = if present { "" } else { "?" };
        let _ = writeln!(
            output,
            "{indent}{}{optional}: {};",
            property_name(name),
            render(property, depth + 1, response)
        );
    }
    if let Some(index) = index {
        let _ = writeln!(output, "{indent}[key: string]: {index};");
    }
    output.push_str(&"  ".repeat(depth));
    output.push('}');
    output
}

fn is_nullable(schema: &Value) -> bool {
    let null_type = |schema: &Value| match schema.get("type") {
        Some(Value::String(kind)) => kind == "null",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "null"),
        _ => false,
    };
    null_type(schema)
        || ["anyOf", "oneOf"].iter().any(|keyword| {
            schema
                .get(*keyword)
                .and_then(Value::as_array)
                .is_some_and(|variants| variants.iter().any(null_type))
        })
}

fn literal(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

fn union(parts: impl Iterator<Item = String>) -> String {
    dedup_join(parts.collect(), " | ")
}

fn dedup_join(parts: Vec<String>, separator: &str) -> String {
    let mut unique = Vec::with_capacity(parts.len());
    for part in parts {
        if !unique.contains(&part) {
            unique.push(part);
        }
    }
    if unique.is_empty() {
        "never".to_owned()
    } else {
        unique.join(separator)
    }
}

/// Wrap unions and intersections so `[]`, `|` and `&` keep their meaning.
fn parenthesize(rendered: String) -> String {
    if (rendered.contains(" | ") || rendered.contains(" & ")) && !rendered.starts_with('{') {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn identifier(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn property_name(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if plain {
        name.to_owned()
    } else {
        Value::String(name.to_owned()).to_string()
    }
}

fn push_doc_comment(output: &mut String, description: &str, indent: &str) {
    let text = description.replace("*/", "* /");
    let _ = writeln!(
        output,
        "{indent}/** {} */",
        text.lines().collect::<Vec<_>>().join(" ")
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::http_typescript;

    #[test]
    fn renders_objects_nullable_unions_references_and_maps() {
        let document = json!({
            "paths": {"/tracks/{id}": {
                "get": {"responses": {"200": {"content": {"application/json": {
                    "schema": {"$ref": "#/components/schemas/Track"}
                }}}}},
                "patch": {"requestBody": {"content": {"application/json": {
                    "schema": {"$ref": "#/components/schemas/TrackPatch"}
                }}}}
            }},
            "components": {"schemas": {
                "Track": {
                    "type": "object",
                    "required": ["id", "year"],
                    "properties": {
                        "id": {"type": "integer"},
                        "year": {"type": ["integer", "null"]},
                        "kind": {"enum": ["tag", "rename"]},
                        "tags": {"type": "array", "items": {"$ref": "#/components/schemas/Tag"}},
                        "extra-field": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                        "scores": {"type": "object", "additionalProperties": {"type": "number"}}
                    }
                },
                "TrackPatch": {"type": "object", "properties": {"title": {"type": ["string", "null"]}}},
                "Tag": {"type": "string"},
                "app__api__Move": {"type": "object", "additionalProperties": false, "properties": {}}
            }}
        });
        assert_eq!(
            http_typescript(&document),
            "// Generated from the Rust OpenAPI document by `music-cli contracts export`.\n\
             // Do not edit by hand.\n\
             \n\
             export type Tag = string;\n\
             \n\
             export type Track = {\n  \
             \"extra-field\": string | null;\n  \
             id: number;\n  \
             kind?: \"tag\" | \"rename\";\n  \
             scores?: Record<string, number>;\n  \
             tags?: Tag[];\n  \
             year: number | null;\n\
             };\n\
             \n\
             export type TrackPatch = {\n  \
             title?: string | null;\n\
             };\n\
             \n\
             export type app__api__Move = Record<string, never>;\n"
        );
    }
}

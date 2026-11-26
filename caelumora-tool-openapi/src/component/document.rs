//! The reading of an OpenAPI 3.x document (issue 252): the operations chosen of it, each as a
//! tool offers it - what it does, its arguments as one object - and as a call sends it - its
//! method, its path, where each argument goes. The document is read as JSON, by the parts needed:
//! its paths, their operations and parameters, a JSON request body, `$ref`s into `components`.
//!
//! The arguments of an operation are its parameters by their names - without the `[]` a name may
//! end in (`users[]`), which a call sends again - and its request body as one property of its own,
//! `body`, whatever the body is: no field of a body can collide with a parameter. What a chosen
//! operation needs that cannot be given - a parameter in a header or a cookie, or without a name
//! or a place, a body that is required and no JSON, a parameter named `body`, two arguments of one
//! name - fails the reading, as an operation does that the document lacks: it was chosen, and
//! whoever chose it learns why it is not offered.

use caelumora_domain::tool::ToolParameters;
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

/// The property that holds an operation's request body.
pub(super) const BODY: &str = "body";

/// An operation of the document as it was read: what a tool offers of it, and the operation its
/// calls send.
#[derive(Debug, PartialEq)]
pub(super) struct Offered {
    pub(super) operation: Operation,
    /// What it does, written for a model: its summary and its description.
    pub(super) text: String,
    /// Its arguments, as one object.
    pub(super) arguments: ToolParameters,
}

/// An operation of the document, as a call sends it.
#[derive(Debug, PartialEq)]
pub(super) struct Operation {
    /// Its `operationId`.
    pub(super) id: String,
    pub(super) method: Method,
    /// Its path below the endpoint, with a `{name}` where a parameter goes.
    pub(super) path: String,
    /// Its parameters, in the path and in the query.
    pub(super) parameters: Vec<Parameter>,
    /// Whether it takes a body, the argument `body`.
    pub(super) body: bool,
}

/// A parameter of an operation: the argument it is given by, and how it is sent.
#[derive(Debug, PartialEq)]
pub(super) struct Parameter {
    /// The argument's name: the parameter's, without a `[]` it ends in.
    pub(super) name: String,
    /// The name it is sent by, as the document writes it.
    pub(super) sent_as: String,
    pub(super) place: Place,
}

/// Where a parameter is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    Path,
    Query,
}

/// Why the operations chosen cannot be read of a document. It names no content of the document
/// but the names of operations and parameters.
#[derive(Debug, thiserror::Error)]
pub(super) enum DocumentError {
    #[error("the document is no OpenAPI 3.x document")]
    NotOpenApi,
    #[error("the document lacks the operations {0}")]
    Missing(String),
    #[error("the operation '{operation}' cannot be offered: {why}")]
    Unsupported { operation: String, why: String },
}

impl Operation {
    /// Whether it only reads, as HTTP defines its method: `GET` and `HEAD`. A server that
    /// changes something on a `GET` would be wrong, which `"unless_read_only"` trusts it is not.
    pub(super) fn read_only(&self) -> bool {
        self.method == Method::GET || self.method == Method::HEAD
    }
}

/// The operations `chosen` of `document`, in their order.
///
/// # Errors
/// `NotOpenApi` for a document of no OpenAPI 3.x, `Missing` for chosen operations it lacks,
/// `Unsupported` for one that needs what cannot be given.
pub(super) fn operations<'a>(
    document: &Value,
    chosen: impl IntoIterator<Item = &'a String>,
) -> Result<Vec<Offered>, DocumentError> {
    let version = document.get("openapi").and_then(Value::as_str);
    let paths = document.get("paths").and_then(Value::as_object);
    let (Some(paths), true) = (paths, version.is_some_and(|v| v.starts_with("3."))) else {
        return Err(DocumentError::NotOpenApi);
    };
    let chosen: Vec<&String> = chosen.into_iter().collect();
    let mut found = HashMap::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else { continue };
        let shared = item.get("parameters");
        for (key, operation) in item {
            let Some(method) = method_of(key) else { continue };
            let id = operation.get("operationId").and_then(Value::as_str);
            let Some(id) = id.filter(|id| chosen.iter().any(|chosen| chosen == id)) else {
                continue;
            };
            let read = OperationReading { document, id, path, method, shared, operation };
            found.insert(id, read.operation()?);
        }
    }
    let missing: Vec<String> = chosen
        .iter()
        .filter(|id| !found.contains_key(id.as_str()))
        .map(|id| format!("'{id}'"))
        .collect();
    if !missing.is_empty() {
        return Err(DocumentError::Missing(missing.join(", ")));
    }
    Ok(chosen.iter().filter_map(|id| found.remove(id.as_str())).collect())
}

/// The method a path item names by `key`; `None` for what is no method - its parameters, its
/// summary.
fn method_of(key: &str) -> Option<Method> {
    match key {
        "get" => Some(Method::GET),
        "put" => Some(Method::PUT),
        "post" => Some(Method::POST),
        "delete" => Some(Method::DELETE),
        "options" => Some(Method::OPTIONS),
        "head" => Some(Method::HEAD),
        "patch" => Some(Method::PATCH),
        "trace" => Some(Method::TRACE),
        _ => None,
    }
}

/// `value`, or what its `$ref` points to in `document`, if it is a reference into it.
fn resolved<'a>(document: &'a Value, value: &'a Value) -> &'a Value {
    let pointer = value.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#'));
    pointer.and_then(|pointer| document.pointer(pointer)).unwrap_or(value)
}

/// `schema` - a string's, if there is none - with `description` written into it, which a
/// parameter or a body writes beside its schema, so that the model is told it.
fn described(schema: Option<&Value>, description: Option<&Value>) -> Value {
    let mut schema = schema.cloned().unwrap_or_else(|| json!({ "type": "string" }));
    if let (Some(fields), Some(description)) = (schema.as_object_mut(), description) {
        fields.insert("description".to_string(), description.clone());
    }
    schema
}

/// Whether `media_type` - with parameters, in any case - is JSON: `application/json`, or a type of
/// its own written in JSON (`application/merge-patch+json`).
pub(super) fn is_json(media_type: &str) -> bool {
    let media_type = media_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

/// The schema of a JSON body of `content`, the media types of a request body; `{}` for JSON of no
/// schema, `None` for a body that is no JSON.
fn json_schema_of(content: Option<&Value>) -> Option<Value> {
    let (_, media) = content?.as_object()?.iter().find(|(media_type, _)| is_json(media_type))?;
    Some(media.get("schema").cloned().unwrap_or_else(|| json!({})))
}

/// The parameters of `list`, a list of them, each resolved in `document`.
fn listed<'a>(document: &'a Value, list: Option<&'a Value>) -> Vec<&'a Value> {
    let parameters = list.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    parameters.iter().map(|parameter| resolved(document, parameter)).collect()
}

/// What tells one parameter of an operation from another: its name and where it is sent.
fn key_of(parameter: &Value) -> (Option<&str>, Option<&str>) {
    let field = |name: &str| parameter.get(name).and_then(Value::as_str);
    (field("name"), field("in"))
}

/// One operation of a document being read.
struct OperationReading<'a> {
    document: &'a Value,
    id: &'a str,
    path: &'a str,
    method: Method,
    /// The parameters its path item gives all its operations.
    shared: Option<&'a Value>,
    operation: &'a Value,
}

impl<'a> OperationReading<'a> {
    /// The operation read.
    ///
    /// # Errors
    /// `Unsupported` if it needs what cannot be given.
    fn operation(self) -> Result<Offered, DocumentError> {
        let mut properties = HashMap::new();
        let mut required = Vec::new();
        let mut parameters = Vec::new();
        for declared in self.parameters() {
            let parameter = self.parameter(declared)?;
            if parameter.name == BODY {
                let why = format!("its parameter '{}' is named as its body", parameter.sent_as);
                return Err(self.unsupported(why));
            }
            if properties.contains_key(&parameter.name) {
                let why = format!("two of its arguments are named '{}'", parameter.name);
                return Err(self.unsupported(why));
            }
            let schema = described(declared.get("schema"), declared.get("description"));
            let read = ToolParameters::from_json_schema(&schema, self.document);
            let is_required = declared.get("required").and_then(Value::as_bool) == Some(true);
            if parameter.place == Place::Path || is_required {
                required.push(parameter.name.clone());
            }
            properties.insert(parameter.name.clone(), read);
            parameters.push(parameter);
        }
        let body = self.body()?;
        if let Some((schema, is_required)) = &body {
            properties
                .insert(BODY.to_string(), ToolParameters::from_json_schema(schema, self.document));
            if *is_required {
                required.push(BODY.to_string());
            }
        }
        let text: Vec<&str> = ["summary", "description"]
            .iter()
            .filter_map(|key| self.operation.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect();
        let operation = Operation {
            id: self.id.to_string(),
            method: self.method,
            path: self.path.to_string(),
            parameters,
            body: body.is_some(),
        };
        let arguments = ToolParameters::Object { properties, required };
        Ok(Offered { operation, text: text.join("\n\n"), arguments })
    }

    /// Its parameters, resolved: its own, and those of its path item it does not declare again.
    fn parameters(&self) -> Vec<&'a Value> {
        let own = listed(self.document, self.operation.get("parameters"));
        let declared: BTreeSet<_> = own.iter().map(|parameter| key_of(parameter)).collect();
        let shared = listed(self.document, self.shared);
        let shared = shared.into_iter().filter(|shared| !declared.contains(&key_of(shared)));
        own.iter().copied().chain(shared).collect()
    }

    /// The parameter `declared` sends.
    ///
    /// # Errors
    /// `Unsupported` for one without a name, or not sent in the path or the query.
    fn parameter(&self, declared: &Value) -> Result<Parameter, DocumentError> {
        let Some(sent_as) = declared.get("name").and_then(Value::as_str) else {
            return Err(self.unsupported("a parameter has no name".to_string()));
        };
        let place = match declared.get("in").and_then(Value::as_str) {
            Some("path") => Place::Path,
            Some("query") => Place::Query,
            Some(place) => {
                let why = format!("its parameter '{sent_as}' is sent in the {place}");
                return Err(self.unsupported(why));
            }
            None => {
                let why = format!("its parameter '{sent_as}' says not where it is sent");
                return Err(self.unsupported(why));
            }
        };
        let name = sent_as.strip_suffix("[]").unwrap_or(sent_as).to_string();
        Ok(Parameter { name, sent_as: sent_as.to_string(), place })
    }

    /// The schema of its body, its description written into it, and whether the body is
    /// required; `None` without a body, or for one that is no JSON and not required.
    ///
    /// # Errors
    /// `Unsupported` for a body that is required and no JSON.
    fn body(&self) -> Result<Option<(Value, bool)>, DocumentError> {
        let Some(body) = self.operation.get("requestBody") else { return Ok(None) };
        let body = resolved(self.document, body);
        let is_required = body.get("required").and_then(Value::as_bool) == Some(true);
        match json_schema_of(body.get("content")) {
            Some(schema) => {
                Ok(Some((described(Some(&schema), body.get("description")), is_required)))
            }
            None if is_required => Err(self.unsupported("its body is no JSON".to_string())),
            None => Ok(None),
        }
    }

    fn unsupported(&self, why: String) -> DocumentError {
        DocumentError::Unsupported { operation: self.id.to_string(), why }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of Kimai's shape: absolute paths, `[]` names, `"items": []`, a body of
    /// `components`, a body that is an array.
    fn kimai() -> Value {
        let users = json!({
            "name": "users[]", "in": "query", "description": "User IDs",
            "schema": { "type": "array", "items": [] }
        });
        let order = json!({
            "name": "order", "in": "query", "schema": { "type": "string", "pattern": "ASC|DESC" }
        });
        let form = json!({ "$ref": "#/components/schemas/TimesheetEditForm" });
        let user = json!({
            "name": "id", "in": "path", "required": true, "description": "User ID",
            "schema": { "type": "integer" }
        });
        let preference =
            json!({ "type": "object", "properties": { "name": { "type": "string" } } });
        let preferences = json!({ "type": "array", "items": preference });
        json!({
            "openapi": "3.0.0",
            "info": { "title": "Kimai", "description": "Time tracking." },
            "paths": {
                "/api/timesheets": {
                    "get": {
                        "operationId": "get_timesheets",
                        "summary": "Returns a collection of timesheet records",
                        "parameters": [users, order]
                    },
                    "post": {
                        "operationId": "post_timesheet",
                        "summary": "Creates a new timesheet record",
                        "description": "Creates it for the current user.",
                        "requestBody": {
                            "required": true,
                            "content": { "application/json": { "schema": form } }
                        }
                    }
                },
                "/api/timesheets/{id}/stop": {
                    "parameters": [{ "name": "id", "in": "path", "schema": { "type": "string" } }],
                    "patch": {
                        "operationId": "patch_stop_timesheet", "summary": "Stops a timesheet"
                    }
                },
                "/api/users/{id}/preferences": {
                    "patch": {
                        "operationId": "patch_user_preferences",
                        "parameters": [user],
                        "requestBody": {
                            "content": { "application/json": { "schema": preferences } }
                        }
                    }
                },
                "/api/export": { "get": { "summary": "Exports, without an operationId" } }
            },
            "components": {
                "schemas": {
                    "TimesheetEditForm": {
                        "type": "object",
                        "required": ["project"],
                        "properties": {
                            "project": { "type": "integer", "description": "Project ID" }
                        }
                    }
                }
            }
        })
    }

    fn chosen(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn read(document: &Value, ids: &[&str]) -> Result<Vec<Offered>, DocumentError> {
        operations(document, &chosen(ids))
    }

    fn string(description: Option<&str>) -> ToolParameters {
        ToolParameters::String { description: description.map(str::to_string), language: None }
    }

    #[test]
    fn the_operations_chosen_are_read_in_their_order_with_parameters_by_their_names() {
        let document = kimai();
        let read = read(&document, &["patch_stop_timesheet", "get_timesheets"]).unwrap();
        let ids: Vec<&str> = read.iter().map(|offered| offered.operation.id.as_str()).collect();
        assert_eq!(ids, ["patch_stop_timesheet", "get_timesheets"]);
        let [stop, timesheets] = read.as_slice() else { panic!("two") };
        assert_eq!(
            *stop,
            Offered {
                operation: Operation {
                    id: "patch_stop_timesheet".to_string(),
                    method: Method::PATCH,
                    path: "/api/timesheets/{id}/stop".to_string(),
                    parameters: vec![Parameter {
                        name: "id".to_string(),
                        sent_as: "id".to_string(),
                        place: Place::Path,
                    }],
                    body: false,
                },
                text: "Stops a timesheet".to_string(),
                arguments: ToolParameters::Object {
                    properties: HashMap::from([("id".to_string(), string(None))]),
                    required: vec!["id".to_string()],
                },
            },
            "a parameter of its path item, required as every one in a path"
        );
        assert!(!stop.operation.read_only() && timesheets.operation.read_only());
        let users = &timesheets.operation.parameters[0];
        assert_eq!((users.name.as_str(), users.sent_as.as_str()), ("users", "users[]"));
        let ToolParameters::Object { properties, required } = &timesheets.arguments else {
            panic!("an object: {:?}", timesheets.arguments)
        };
        assert!(required.is_empty(), "neither is required");
        assert_eq!(
            properties["users"],
            ToolParameters::Array {
                items: Box::new(string(Some("Array item"))),
                description: Some("User IDs.".to_string()),
            },
            "described as the parameter is, its items no schema"
        );
        assert_eq!(properties["order"], string(Some("Matching: ASC|DESC")));
    }

    #[test]
    fn a_body_is_one_argument_of_its_own_whatever_it_is() {
        let document = kimai();
        let read = read(&document, &["post_timesheet", "patch_user_preferences"]).unwrap();
        let [post, preferences] = read.as_slice() else { panic!("two") };
        assert!(post.operation.body && post.operation.parameters.is_empty());
        assert_eq!(post.text, "Creates a new timesheet record\n\nCreates it for the current user.");
        let ToolParameters::Object { properties, required } = &post.arguments else { panic!() };
        assert_eq!(required, &[BODY]);
        let ToolParameters::Object { properties: fields, required } = &properties[BODY] else {
            panic!("the form, where its reference points: {:?}", properties[BODY])
        };
        assert_eq!(required, &["project"]);
        assert!(fields.contains_key("project"));
        let ToolParameters::Object { properties, required } = &preferences.arguments else {
            panic!()
        };
        assert_eq!(required, &["id"], "an optional body");
        assert!(matches!(properties[BODY], ToolParameters::Array { .. }), "{:?}", properties[BODY]);
        assert!(matches!(properties["id"], ToolParameters::Number { .. }));
    }

    #[test]
    fn what_the_document_lacks_or_is_not_fails_the_reading_naming_no_content() {
        let document = kimai();
        let missing = read(&document, &["get_timesheets", "delete_all", "get_export"]).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "the document lacks the operations 'delete_all', 'get_export'"
        );
        let swagger = json!({ "swagger": "2.0", "paths": {} });
        assert!(matches!(read(&swagger, &["a"]), Err(DocumentError::NotOpenApi)));
        assert!(matches!(read(&json!(["openapi"]), &["a"]), Err(DocumentError::NotOpenApi)));
    }

    #[test]
    fn an_operation_that_needs_what_cannot_be_given_fails_the_reading() {
        let document = |parameters: Value, body: Option<Value>| {
            let mut operation = json!({ "operationId": "op", "parameters": parameters });
            if let Some(body) = body {
                operation["requestBody"] = body;
            }
            json!({ "openapi": "3.1.0", "paths": { "/x": { "post": operation } } })
        };
        let refused = |document: Value| read(&document, &["op"]).unwrap_err().to_string();
        let header = json!([{ "name": "X-Key", "in": "header" }]);
        assert_eq!(
            refused(document(header, None)),
            "the operation 'op' cannot be offered: its parameter 'X-Key' is sent in the header"
        );
        let nowhere = json!([{ "name": "id" }]);
        assert!(refused(document(nowhere, None)).contains("'id' says not where it is sent"));
        let nameless = json!([{ "in": "query" }]);
        assert!(refused(document(nameless, None)).contains("a parameter has no name"));
        let twice = json!([{ "name": "id", "in": "query" }, { "name": "id[]", "in": "query" }]);
        assert!(refused(document(twice, None)).contains("two of its arguments are named 'id'"));
        let body = json!([{ "name": "body", "in": "query" }]);
        assert!(refused(document(body, None)).contains("'body' is named as its body"), "always");
        let upload = json!({ "required": true, "content": { "multipart/form-data": {} } });
        assert!(refused(document(json!([]), Some(upload))).contains("its body is no JSON"));
        let optional = json!({ "content": { "text/plain": {} } });
        let read = read(&document(json!([]), Some(optional)), &["op"]).unwrap();
        assert!(!read[0].operation.body, "an optional body that is no JSON is left out");
    }

    #[test]
    fn an_operation_declares_again_what_its_path_item_gives_and_reads_references() {
        let limit = json!({ "name": "limit", "in": "query", "schema": { "type": "integer" } });
        let patch = json!({ "schema": { "type": "object", "properties": {} } });
        let document = json!({
            "openapi": "3.0.3",
            "paths": { "/items/{id}": {
                "parameters": [
                    { "name": "id", "in": "path", "description": "shared" },
                    { "$ref": "#/components/parameters/Limit" }
                ],
                "get": {
                    "operationId": "get_item",
                    "parameters": [{ "name": "id", "in": "path", "description": "own" }],
                    "requestBody": { "$ref": "#/components/requestBodies/Item" }
                }
            } },
            "components": {
                "parameters": { "Limit": limit },
                "requestBodies": {
                    "Item": { "content": { "application/merge-patch+json": patch } }
                }
            }
        });
        let read = read(&document, &["get_item"]).unwrap();
        let ToolParameters::Object { properties, .. } = &read[0].arguments else { panic!() };
        assert_eq!(properties["id"], string(Some("own.")), "its own over the path item's");
        assert!(matches!(properties["limit"], ToolParameters::Number { .. }), "where it points");
        assert!(read[0].operation.body, "JSON of a type of its own, where its reference points");
    }

    #[test]
    fn a_media_type_is_json_as_written_or_of_a_type_of_its_own() {
        for json in ["application/json", "Application/JSON; charset=utf-8", "application/ld+json"] {
            assert!(is_json(json), "{json}");
        }
        for other in ["text/plain", "application/pdf", "", "application/jsonx"] {
            assert!(!is_json(other), "{other}");
        }
    }
}

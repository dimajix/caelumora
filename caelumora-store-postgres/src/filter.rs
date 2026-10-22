//! A filter as a condition of SQL (issue 03): what of a query the database
//! judges, so that a scan reads what meets it rather than every document of
//! the kind.

use caelumora_model::store::query::{FieldPath, FieldValue, Filter};
use serde_json::{Map, Value};
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;
use sqlx::Postgres;

/// What a filter is translated into: a condition of SQL over a column of
/// JSON - the body of a document, an item of a history - that holds for every document that meets the filter, and - where
/// SQL cannot tell, as for names compared folded (`NamePrefix`, `Contains`) -
/// for others besides, which the store judges again. With it, what it binds,
/// numbered after the parameters of the query before it.
///
/// Each condition is translated as `Filter::matches` judges it, of a
/// document as serde writes it - which every document in a table is, since
/// the store alone writes them: an id as its hyphenated form in small
/// letters. A field that is not there is `NULL` in SQL; every condition is
/// made `false` then, as `matches` judges it, so that one negated holds.
/// What can be asked as containment (`@>`) besides is asked so, too, which
/// a GIN index of the column answers.
#[derive(Debug)]
pub(crate) struct Condition {
    pub(crate) sql: String,
    binds: Vec<Bound>,
    /// The number of the first parameter it binds.
    first: usize,
    /// How many elements of collections it names (`Filter::Any`).
    elements: usize,
}

/// A value a condition binds.
#[derive(Debug, Clone, PartialEq)]
enum Bound {
    Json(Value),
    Text(String),
    /// The path of a field, as `#>` takes it.
    Path(Vec<String>),
}

impl Condition {
    /// The condition of `filter` on the column `column`, its parameters
    /// numbered from `first`.
    pub(crate) fn of(filter: &Filter<FieldPath>, column: &str, first: usize) -> Self {
        let mut condition = Self { sql: String::new(), binds: Vec::new(), first, elements: 0 };
        let (judged, _) = condition.judging(filter, column);
        condition.sql = match contained(filter) {
            Some(contained) => format!("{column} @> {} AND {judged}", condition.bind(Bound::Json(contained))),
            None => judged,
        };
        condition
    }

    /// `query`, which binds the parameters before `first`, with what the
    /// condition binds bound after them.
    pub(crate) fn bound<'q, O>(&self, mut query: QueryAs<'q, Postgres, O, PgArguments>) -> QueryAs<'q, Postgres, O, PgArguments> {
        for bound in &self.binds {
            query = match bound {
                Bound::Json(value) => query.bind(value.clone()),
                Bound::Text(text) => query.bind(text.clone()),
                Bound::Path(path) => query.bind(path.clone()),
            };
        }
        query
    }

    /// The parameter that binds `value`.
    fn bind(&mut self, value: Bound) -> String {
        self.binds.push(value);
        format!("${}", self.first + self.binds.len() - 1)
    }

    /// What lies at `path` in the JSON `at`.
    fn at(&mut self, at: &str, path: FieldPath) -> String {
        match path.is_empty() {
            true => at.to_string(),
            false => format!("({at} #> {}::text[])", self.bind(Bound::Path(path.iter().map(ToString::to_string).collect()))),
        }
    }

    /// The condition of `filter` on the JSON `at`, and whether it holds for
    /// exactly what meets the filter - else for more besides.
    fn judging(&mut self, filter: &Filter<FieldPath>, at: &str) -> (String, bool) {
        match filter {
            Filter::Eq(field, value) => {
                let found = self.at(at, field);
                (format!("COALESCE({}, false)", self.meeting(value, &found)), true)
            }
            Filter::In(field, values) => {
                let found = self.at(at, field);
                let any: Vec<String> = values.iter().map(|value| self.meeting(value, &found)).collect();
                (format!("COALESCE({}, false)", joined(&any, "OR", "false")), true)
            }
            Filter::IsNull(field) => {
                let found = self.at(at, field);
                (format!("({found} IS NULL OR {found} = 'null'::jsonb)"), true)
            }
            Filter::And(conditions) => self.joining(conditions, at, "AND", "true"),
            Filter::Or(conditions) => self.joining(conditions, at, "OR", "false"),
            // Not what holds for more besides holds for fewer: it is left to
            // the store, and what it would have bound is taken back.
            Filter::Not(condition) => {
                let bound = self.binds.len();
                match self.judging(condition, at) {
                    (judged, true) => (format!("NOT {judged}"), true),
                    (_, false) => {
                        self.binds.truncate(bound);
                        ("true".to_string(), false)
                    }
                }
            }
            Filter::Any(collection, condition) => {
                let found = self.at(at, collection);
                self.elements += 1;
                let element = format!("e{}", self.elements);
                let (judged, exact) = self.judging(condition, &format!("{element}.v"));
                let elements = format!("CASE WHEN jsonb_typeof({found}) = 'array' THEN {found} ELSE '[]'::jsonb END");
                (format!("EXISTS (SELECT 1 FROM jsonb_array_elements({elements}) AS {element}(v) WHERE {judged})"), exact)
            }
            Filter::NamePrefix(_, _) | Filter::Contains(_, _) => ("true".to_string(), false),
        }
    }

    /// `conditions` joined by `operator`, `empty` if there are none.
    fn joining(&mut self, conditions: &[Filter<FieldPath>], at: &str, operator: &str, empty: &str) -> (String, bool) {
        let judged: Vec<(String, bool)> = conditions.iter().map(|condition| self.judging(condition, at)).collect();
        let exact = judged.iter().all(|(_, exact)| *exact);
        let sql: Vec<String> = judged.into_iter().map(|(sql, _)| sql).collect();
        (joined(&sql, operator, empty), exact)
    }

    /// Whether the JSON `found` meets `value`, as `FieldValue::matches`
    /// judges it; `NULL` if `found` is.
    fn meeting(&mut self, value: &FieldValue, found: &str) -> String {
        match value {
            FieldValue::Uuid(id) => format!("{found} = {}", self.bind(Bound::Json(Value::String(id.to_string())))),
            FieldValue::Text(text) => format!("{found} = {}", self.bind(Bound::Json(Value::String(text.clone())))),
            FieldValue::Bool(flag) => format!("{found} = {}", self.bind(Bound::Json(Value::Bool(*flag)))),
            FieldValue::Tagged(tag, inner) => {
                let held = format!("({found} -> {}::text)", self.bind(Bound::Text(tag.to_string())));
                self.meeting(inner, &held)
            }
            FieldValue::Variant(tag) => {
                let unit = self.bind(Bound::Json(Value::String(tag.to_string())));
                let key = self.bind(Bound::Text(tag.to_string()));
                format!("({found} = {unit} OR (jsonb_typeof({found}) = 'object' AND {found} ? {key}::text))")
            }
        }
    }
}

/// `conditions`, each in parentheses, joined by `operator`; `empty` if there
/// are none.
fn joined(conditions: &[String], operator: &str, empty: &str) -> String {
    match conditions {
        [] => empty.to_string(),
        [one] => one.clone(),
        _ => format!("({})", conditions.join(&format!(" {operator} "))),
    }
}

/// What every document that meets `filter` contains (`@>`), if a filter of
/// its form can say: the fields it compares with values, the elements of
/// collections that one of it meets. Every document that meets the filter
/// contains it - and others may, which the condition judges.
fn contained(filter: &Filter<FieldPath>) -> Option<Value> {
    match filter {
        Filter::Eq(field, value) => Some(wrapped(field, as_contained(value)?)),
        Filter::Any(collection, condition) => Some(wrapped(collection, Value::Array(vec![contained(condition)?]))),
        Filter::And(conditions) => conditions.iter().filter_map(contained).reduce(|all, one| merged(all.clone(), one).unwrap_or(all)),
        _ => None,
    }
}

/// `value` as a document that meets it contains it, if it can be said: not
/// a variant whatever it holds, which is a string or a key.
fn as_contained(value: &FieldValue) -> Option<Value> {
    match value {
        FieldValue::Uuid(id) => Some(Value::String(id.to_string())),
        FieldValue::Text(text) => Some(Value::String(text.clone())),
        FieldValue::Bool(flag) => Some(Value::Bool(*flag)),
        FieldValue::Tagged(tag, inner) => Some(Value::Object(Map::from_iter([(tag.to_string(), as_contained(inner)?)]))),
        FieldValue::Variant(_) => None,
    }
}

/// `value` at `path`: `{"a": {"b": value}}` of `["a", "b"]`.
fn wrapped(path: FieldPath, value: Value) -> Value {
    path.iter().rev().fold(value, |inner, key| Value::Object(Map::from_iter([(key.to_string(), inner)])))
}

/// What contains both `one` and `other`, if one value does: the keys of
/// both objects, the elements of both collections. `None` where they hold
/// two values at one place - no document meets both then, which the
/// condition judges.
fn merged(one: Value, other: Value) -> Option<Value> {
    match (one, other) {
        (Value::Object(mut one), Value::Object(other)) => {
            for (key, value) in other {
                let both = match one.remove(&key) {
                    Some(mine) => merged(mine, value)?,
                    None => value,
                };
                one.insert(key, both);
            }
            Some(Value::Object(one))
        }
        (Value::Array(mut one), Value::Array(other)) => {
            one.extend(other);
            Some(Value::Array(one))
        }
        (one, other) => (one == other).then_some(one),
    }
}

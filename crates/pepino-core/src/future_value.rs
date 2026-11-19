use ciborium::Value;

use crate::{EdgeKind, Status};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnknownValue {
    Status(String),
    EdgeKind(String),
    FieldValue(String),
}

// Substitute only unrecognized discriminants so the ordinary decoder can still
// detect corruption in the parts of the payload this version understands.
pub(crate) fn substitute_unknown_values(event: &mut Value) -> Vec<UnknownValue> {
    let mut unknown = Vec::new();
    let Some((tag, payload)) = variant_payload(event) else {
        return unknown;
    };
    match tag {
        "task_created" => {
            if let Some(fields) = member(payload, "fields") {
                if let Some(value) = member(fields, "status") {
                    status(value, &mut unknown);
                }
                if let Some(custom) = member(fields, "custom").and_then(|v| untag(v).as_map_mut()) {
                    for (_, value) in custom {
                        field_value(value, &mut unknown);
                    }
                }
            }
        }
        "task_field_set" => {
            if let Some(value) = member(payload, "value") {
                field_value(value, &mut unknown);
            }
        }
        "edge_added" | "edge_removed" => {
            if let Some(value) = member(payload, "kind") {
                let value = untag(value);
                if let Some(tag) = value.as_text()
                    && value.deserialized::<EdgeKind>().is_err()
                {
                    unknown.push(UnknownValue::EdgeKind(tag.into()));
                    *value = "relates".into();
                }
            }
        }
        _ => {}
    }
    unknown
}

fn status(value: &mut Value, unknown: &mut Vec<UnknownValue>) {
    let value = untag(value);
    if let Some(tag) = value.as_text()
        && value.deserialized::<Status>().is_err()
    {
        unknown.push(UnknownValue::Status(tag.into()));
        *value = "todo".into();
    }
}

fn field_value(value: &mut Value, unknown: &mut Vec<UnknownValue>) {
    let value = untag(value);
    let tag = match value {
        Value::Text(tag) => tag.as_str(),
        Value::Map(entries) => match entries.as_slice() {
            [(Value::Text(tag), _)] => tag.as_str(),
            _ => return,
        },
        _ => return,
    };
    match tag {
        "status" => {
            if let Some((_, payload)) = variant_payload(value) {
                status(payload, unknown);
            }
        }
        "text" | "integer" | "date" | "unset" => {}
        _ => {
            unknown.push(UnknownValue::FieldValue(tag.into()));
            *value = "unset".into();
        }
    }
}

fn member<'a>(value: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    untag(value)
        .as_map_mut()?
        .iter_mut()
        .find_map(|(key, value)| (key.as_text() == Some(name)).then_some(value))
}

fn variant_payload(value: &mut Value) -> Option<(&str, &mut Value)> {
    let [(Value::Text(tag), payload)] = untag(value).as_map_mut()?.as_mut_slice() else {
        return None;
    };
    Some((tag, payload))
}

fn untag(mut value: &mut Value) -> &mut Value {
    while let Value::Tag(_, inner) = value {
        value = inner;
    }
    value
}

//! What the modules for the compositors' own IPC share: handing events to the UI threads that
//! listen, and reading JSON that differs a little between versions.

pub(crate) use crate::listeners::Listeners;
use serde_json::Value;

pub(crate) fn int(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(|x| x.as_i64().or_else(|| x.as_bool().map(i64::from)))
        .unwrap_or(0)
}

pub(crate) fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// A flag, which some versions give as a number.
pub(crate) fn flag(v: &Value, key: &str) -> bool {
    v.get(key)
        .is_some_and(|x| x.as_bool().unwrap_or_else(|| x.as_i64().unwrap_or(0) != 0))
}

pub(crate) fn failed(e: impl std::fmt::Display) -> crate::ServiceError {
    crate::ServiceError::Failed(e.to_string())
}

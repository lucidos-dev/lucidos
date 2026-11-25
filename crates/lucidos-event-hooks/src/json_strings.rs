//! A walk over every string a JSON value carries, object keys included, so
//! NUL stripping cannot miss a key. Masking uses the shared masker instead.

use serde_json::{Map, Value};

/// `value` with `f` applied to every string in it, object keys included.
/// `Err` names a key two keys of one object both became, since keeping
/// either would silently drop the other's value.
pub(crate) fn map_strings(value: Value, f: &impl Fn(&str) -> String) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => Value::String(f(&text)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| map_strings(item, f))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(map_object_strings(map, f)?),
        other => other,
    })
}

/// `map` with `f` applied to every key and every string below it.
pub(crate) fn map_object_strings(
    map: Map<String, Value>,
    f: &impl Fn(&str) -> String,
) -> Result<Map<String, Value>, String> {
    let mut mapped = Map::with_capacity(map.len());
    for (key, value) in map {
        let key = f(&key);
        let value = map_strings(value, f)?;
        if mapped.contains_key(&key) {
            return Err(key);
        }
        mapped.insert(key, value);
    }
    Ok(mapped)
}

use serde_json::{Map, Value};

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

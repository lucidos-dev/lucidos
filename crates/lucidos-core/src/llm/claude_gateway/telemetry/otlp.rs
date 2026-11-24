//! An OTLP logs export request (`ExportLogsServiceRequest`), in either OTLP/HTTP
//! encoding.
//!
//! Protobuf is written by hand over the closed subset a batch uses: one
//! resource, one scope, and log records whose values are all strings. That
//! keeps a protobuf code generator out of the build. The field numbers are
//! those of `opentelemetry/proto/logs/v1/logs.proto` and its imports.

use serde_json::{json, Value};

use super::record::LogRecord;
use super::switch::Protocol;

/// `service.name` on the resource.
const SERVICE_NAME: &str = "lucidos";

/// The instrumentation scope every record is emitted under.
const SCOPE_NAME: &str = "lucidos.gateway-telemetry";

/// `SEVERITY_NUMBER_INFO`.
const SEVERITY_INFO: u64 = 9;

const SEVERITY_TEXT: &str = "INFO";

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The request carrying `records`, encoded for `protocol`. `observed_unix_nano`
/// is when the exporter picked them up.
pub(crate) fn encode(
    protocol: Protocol,
    records: &[LogRecord],
    observed_unix_nano: u64,
) -> Vec<u8> {
    match protocol {
        Protocol::Protobuf => protobuf(records, observed_unix_nano),
        Protocol::Json => json_body(records, observed_unix_nano),
    }
}

/// The OTLP JSON mapping: camelCase field names, 64-bit integers as decimal
/// strings, and enums as numbers.
fn json_body(records: &[LogRecord], observed_unix_nano: u64) -> Vec<u8> {
    let string_attributes = |pairs: &[(&str, String)]| -> Value {
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}))
            .collect()
    };
    let log_records: Vec<Value> = records
        .iter()
        .map(|record| {
            json!({
                "timeUnixNano": record.time_unix_nano.to_string(),
                "observedTimeUnixNano": observed_unix_nano.to_string(),
                "severityNumber": SEVERITY_INFO,
                "severityText": SEVERITY_TEXT,
                "body": {"stringValue": record.body()},
                "attributes": string_attributes(&record.attributes),
            })
        })
        .collect();
    let body = json!({
        "resourceLogs": [{
            "resource": {"attributes": string_attributes(&resource_attributes())},
            "scopeLogs": [{
                "scope": {"name": SCOPE_NAME, "version": version()},
                "logRecords": log_records,
            }],
        }],
    });
    serde_json::to_vec(&body).expect("a JSON value always serializes")
}

fn resource_attributes() -> [(&'static str, String); 4] {
    [
        ("service.name", SERVICE_NAME.to_string()),
        ("service.version", version().to_string()),
        ("os.type", os_type(std::env::consts::OS).to_string()),
        ("host.arch", host_arch(std::env::consts::ARCH).to_string()),
    ]
}

/// The OpenTelemetry `os.type` for a Rust target OS, as Claude Code reports it.
fn os_type(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        other => other,
    }
}

/// The OpenTelemetry `host.arch` for a Rust target architecture.
fn host_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Protobuf wire types.
const VARINT: u8 = 0;
const FIXED64: u8 = 1;
const LEN: u8 = 2;

fn protobuf(records: &[LogRecord], observed_unix_nano: u64) -> Vec<u8> {
    let mut resource = Vec::new();
    for (key, value) in resource_attributes() {
        put_message(&mut resource, 1, &key_value(key, &value));
    }

    let mut scope = Vec::new();
    put_bytes(&mut scope, 1, SCOPE_NAME.as_bytes());
    put_bytes(&mut scope, 2, version().as_bytes());

    let mut scope_logs = Vec::new();
    put_message(&mut scope_logs, 1, &scope);
    for record in records {
        put_message(&mut scope_logs, 2, &log_record(record, observed_unix_nano));
    }

    let mut resource_logs = Vec::new();
    put_message(&mut resource_logs, 1, &resource);
    put_message(&mut resource_logs, 2, &scope_logs);

    let mut request = Vec::new();
    put_message(&mut request, 1, &resource_logs);
    request
}

/// A `LogRecord`.
fn log_record(record: &LogRecord, observed_unix_nano: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_fixed64(&mut out, 1, record.time_unix_nano);
    put_varint_field(&mut out, 2, SEVERITY_INFO);
    put_bytes(&mut out, 3, SEVERITY_TEXT.as_bytes());
    put_message(&mut out, 5, &string_value(&record.body()));
    for (key, value) in &record.attributes {
        put_message(&mut out, 6, &key_value(key, value));
    }
    put_fixed64(&mut out, 11, observed_unix_nano);
    out
}

/// A `KeyValue` holding a string.
fn key_value(key: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, key.as_bytes());
    put_message(&mut out, 2, &string_value(value));
    out
}

/// An `AnyValue` holding a string.
fn string_value(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, value.as_bytes());
    out
}

fn put_tag(out: &mut Vec<u8>, field: u64, wire_type: u8) {
    put_varint(out, (field << 3) | u64::from(wire_type));
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_tag(out, field, VARINT);
    put_varint(out, value);
}

fn put_fixed64(out: &mut Vec<u8>, field: u64, value: u64) {
    put_tag(out, field, FIXED64);
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_tag(out, field, LEN);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn put_message(out: &mut Vec<u8>, field: u64, message: &[u8]) {
    put_bytes(out, field, message);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::record::tests::row;
    use super::super::record::{log_record, Identity};
    use super::*;

    /// One decoded protobuf field.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Field {
        Varint(u64),
        Fixed64(u64),
        Bytes(Vec<u8>),
    }

    /// Decode one protobuf message into `(field number, value)` pairs,
    /// refusing anything malformed. Enough to check field numbers.
    pub(crate) fn decode(mut bytes: &[u8]) -> Vec<(u64, Field)> {
        fn varint(bytes: &mut &[u8]) -> u64 {
            let mut value = 0u64;
            for shift in (0..64).step_by(7) {
                let (&byte, rest) = bytes.split_first().expect("a truncated varint");
                *bytes = rest;
                value |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return value;
                }
            }
            panic!("a varint over 64 bits");
        }
        let mut fields = Vec::new();
        while !bytes.is_empty() {
            let tag = varint(&mut bytes);
            let field = match (tag & 7) as u8 {
                VARINT => Field::Varint(varint(&mut bytes)),
                FIXED64 => {
                    let (value, rest) = bytes.split_at(8);
                    bytes = rest;
                    Field::Fixed64(u64::from_le_bytes(value.try_into().unwrap()))
                }
                LEN => {
                    let len = usize::try_from(varint(&mut bytes)).unwrap();
                    let (value, rest) = bytes.split_at(len);
                    bytes = rest;
                    Field::Bytes(value.to_vec())
                }
                other => panic!("unexpected wire type {other}"),
            };
            fields.push((tag >> 3, field));
        }
        fields
    }

    fn bytes_of(fields: &[(u64, Field)], number: u64) -> Vec<Vec<u8>> {
        fields
            .iter()
            .filter(|(n, _)| *n == number)
            .map(|(_, field)| match field {
                Field::Bytes(bytes) => bytes.clone(),
                other => panic!("field {number} is {other:?}"),
            })
            .collect()
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// The `(key, string value)` pairs of repeated `KeyValue` field `number`.
    pub(crate) fn string_pairs(fields: &[(u64, Field)], number: u64) -> Vec<(String, String)> {
        bytes_of(fields, number)
            .iter()
            .map(|kv| {
                let kv = decode(kv);
                let any = decode(&bytes_of(&kv, 2)[0]);
                (text(&bytes_of(&kv, 1)[0]), text(&bytes_of(&any, 1)[0]))
            })
            .collect()
    }

    /// The log records of a protobuf request, each as its decoded fields.
    pub(crate) fn protobuf_records(body: &[u8]) -> Vec<Vec<(u64, Field)>> {
        let request = decode(body);
        let resource_logs = decode(&bytes_of(&request, 1)[0]);
        let scope_logs = decode(&bytes_of(&resource_logs, 2)[0]);
        bytes_of(&scope_logs, 2).iter().map(|r| decode(r)).collect()
    }

    fn two_records() -> Vec<LogRecord> {
        vec![
            log_record(&row(1), None, &Identity::default()),
            log_record(&row(2), None, &Identity::default()),
        ]
    }

    #[test]
    fn protobuf_decodes_as_an_otlp_logs_request() {
        let records = two_records();
        let body = encode(Protocol::Protobuf, &records, 99);
        let request = decode(&body);
        assert_eq!(request.len(), 1, "one ResourceLogs");
        let resource_logs = decode(&bytes_of(&request, 1)[0]);
        let resource = decode(&bytes_of(&resource_logs, 1)[0]);
        assert_eq!(
            string_pairs(&resource, 1),
            resource_attributes()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<Vec<_>>()
        );
        // The OpenTelemetry semantic-convention keys, pinned as a wire contract.
        let keys: Vec<String> = string_pairs(&resource, 1)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            ["service.name", "service.version", "os.type", "host.arch"]
        );
        assert_eq!(string_pairs(&resource, 1)[0].1, "lucidos");
        let scope_logs = decode(&bytes_of(&resource_logs, 2)[0]);
        let scope = decode(&bytes_of(&scope_logs, 1)[0]);
        assert_eq!(text(&bytes_of(&scope, 1)[0]), SCOPE_NAME);

        let decoded = protobuf_records(&body);
        assert_eq!(decoded.len(), 2);
        let first = &decoded[0];
        assert!(first.contains(&(1, Field::Fixed64(records[0].time_unix_nano))));
        assert!(first.contains(&(2, Field::Varint(SEVERITY_INFO))));
        assert!(first.contains(&(11, Field::Fixed64(99))));
        assert_eq!(text(&bytes_of(first, 3)[0]), "INFO");
        let body_value = decode(&bytes_of(first, 5)[0]);
        assert_eq!(
            text(&bytes_of(&body_value, 1)[0]),
            "claude_code.api_request"
        );
        let attributes: Vec<(String, String)> = records[0]
            .attributes
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(string_pairs(first, 6), attributes);
    }

    #[test]
    fn json_follows_the_otlp_json_mapping() {
        let records = two_records();
        let body: Value = serde_json::from_slice(&encode(Protocol::Json, &records, 99)).unwrap();
        let resource_logs = &body["resourceLogs"][0];
        assert_eq!(
            resource_logs["resource"]["attributes"][0],
            json!({"key": "service.name", "value": {"stringValue": "lucidos"}})
        );
        let scope_logs = &resource_logs["scopeLogs"][0];
        assert_eq!(scope_logs["scope"]["name"], SCOPE_NAME);
        let first = &scope_logs["logRecords"][0];
        assert_eq!(first["timeUnixNano"], records[0].time_unix_nano.to_string());
        assert_eq!(first["observedTimeUnixNano"], "99");
        assert_eq!(first["severityNumber"], 9);
        assert_eq!(first["severityText"], "INFO");
        assert_eq!(
            first["body"],
            json!({"stringValue": "claude_code.api_request"})
        );
        assert_eq!(
            first["attributes"][0],
            json!({"key": "event.name", "value": {"stringValue": "api_request"}})
        );
        assert_eq!(scope_logs["logRecords"].as_array().unwrap().len(), 2);
    }

    /// Claude Code's names, so a collector groups both clients' hosts alike.
    #[test]
    fn the_host_is_named_in_opentelemetry_terms() {
        assert_eq!(os_type("macos"), "darwin");
        assert_eq!(os_type("linux"), "linux");
        assert_eq!(host_arch("aarch64"), "arm64");
        assert_eq!(host_arch("x86_64"), "amd64");
    }

    #[test]
    fn a_long_value_takes_a_multi_byte_length() {
        let mut out = Vec::new();
        put_bytes(&mut out, 1, &[b'x'; 300]);
        assert_eq!(decode(&out), vec![(1, Field::Bytes(vec![b'x'; 300]))]);
    }
}

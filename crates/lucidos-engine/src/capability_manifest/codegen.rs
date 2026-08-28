//! Codegen for the manifest-driven agent surfaces (test-only).
//!
//! Mirrors `navigate_targets_codegen` in `llm/tools/misc.rs`: an `#[ignore]`
//! writer test rewrites each generated file, and a non-ignored staleness test
//! fails `cargo test` when the on-disk file drifts from the manifest. The engine
//! crate reaches the sibling crates via `CARGO_MANIFEST_DIR` parents.
//!
//! Generated today:
//! - `crates/lucidos-cli/src/generated/mod.rs` — one `clap::Subcommand` enum +
//!   one gateway-safe `dispatch_<domain>` per `cli = true` domain. The dispatch
//!   routes through the CLI's own `http::client()` / `send_and_print`, so a
//!   generated command can never re-introduce the curl/port/gateway-prefix traps
//!   that motivated the manifest.
//! - `packages/lucidos-sdk/src/generated/capabilities.ts` — the capability table
//!   the SDK parity test checks the hand-written facade against.

use super::*;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn cli_generated_path() -> std::path::PathBuf {
    repo_root().join("crates/lucidos-cli/src/generated/mod.rs")
}

fn sdk_generated_path() -> std::path::PathBuf {
    repo_root().join("packages/lucidos-sdk/src/generated/capabilities.ts")
}

/// kebab/snake → PascalCase (clap variant name; clap renames it back to
/// kebab-case for the subcommand, == `cli_name`).
fn pascal(name: &str) -> String {
    name.split(['-', '_'])
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

fn rust_base_type(ty: ArgType) -> &'static str {
    match ty {
        ArgType::Str => "String",
        ArgType::Int => "i64",
        ArgType::Bool => "bool",
        // A complex arg arrives as a JSON string on the CLI; the dispatch body
        // parses it into a `serde_json::Value` before riding it on the request.
        ArgType::Json => "String",
    }
}

/// clap field type. A *required* bool is a bare flag (`--flag` ⇒ true). An
/// *optional* bool is `Option<bool>` (`--flag true|false`, or absent) so a
/// partial-update op can leave it unchanged rather than silently sending
/// `false` on every call. Other args: required → base, optional → Option.
fn rust_field_type(a: &Arg) -> String {
    match (a.ty, a.required) {
        (ArgType::Bool, true) => "bool".to_string(),
        (ArgType::Bool, false) => "Option<bool>".to_string(),
        (_, true) => rust_base_type(a.ty).to_string(),
        (_, false) => format!("Option<{}>", rust_base_type(a.ty)),
    }
}

// ---------------------------------------------------------------------------
// CLI generation
// ---------------------------------------------------------------------------

/// Pipe generated Rust source through `rustfmt`.
///
/// Panics rather than returning the input unchanged when rustfmt is missing or
/// rejects the source. A silent passthrough would reopen the vise described on
/// `generate_cli_rs` the next time someone regenerated, and the damage would
/// surface as an unrelated `make lint` failure on somebody else's branch.
/// `rust-toolchain.toml` pins `rustfmt` as a component, so its absence means a
/// broken toolchain rather than a supported configuration.
fn rustfmt_source(src: String) -> String {
    let bin = std::env::var("RUSTFMT").unwrap_or_else(|_| "rustfmt".to_string());
    // `--edition` must match `edition` in the workspace root Cargo.toml: rustfmt
    // invoked directly rather than through cargo-fmt defaults to 2015. Drift is
    // caught by the fmt gate itself, which formats this file with the real
    // edition and reports a diff if the two disagree.
    let mut child = std::process::Command::new(&bin)
        .args(["--emit=stdout", "--edition=2021"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            panic!(
                "[Codegen] could not run `{bin}`: {e}. rustfmt is pinned as a component in \
                 rust-toolchain.toml; `rustup component add rustfmt` restores it."
            )
        });

    // The write runs on its own thread because rustfmt streams its result back
    // while we are still feeding it. The generated module is ~44 KB today, close
    // enough to a typical 64 KB pipe buffer that adding a couple of domains would
    // turn a straight-line write into a deadlock.
    let mut stdin = child
        .stdin
        .take()
        .expect("[Codegen] rustfmt stdin not piped");
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        stdin.write_all(src.as_bytes())
    });

    let out = child
        .wait_with_output()
        .expect("[Codegen] failed to wait for rustfmt");
    let wrote = writer
        .join()
        .expect("[Codegen] rustfmt writer thread panicked");
    // Status before the write result, deliberately. The likeliest way this
    // function ever fails is someone changing the templates to emit source
    // rustfmt cannot parse, and a rustfmt that dies mid-read leaves the writer
    // holding a BrokenPipe. Reporting that first would bury the one message
    // that says what is actually wrong with the generated code.
    assert!(
        out.status.success(),
        "[Codegen] rustfmt rejected the generated source ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    wrote.expect("[Codegen] failed to write generated source to rustfmt");
    String::from_utf8(out.stdout).expect("[Codegen] rustfmt emitted invalid UTF-8")
}

/// The generated CLI module, rustfmt-formatted.
///
/// Formatting is load-bearing here, not cosmetic. The file is tracked and
/// `make lint` runs `cargo fmt --all --check` over the tree, which puts it in a
/// vise: `generated_cli_commands_is_up_to_date` demands the on-disk bytes equal
/// this function's output, while the gate demands those bytes are
/// rustfmt-clean. Formatting the file by hand breaks the first, emitting
/// unformatted text breaks the second. Excluding the path is not available:
/// rustfmt's `ignore` key and a module-level `#![rustfmt::skip]` are both
/// nightly-only, and on a stable channel `ignore` merely warns and continues, so
/// the setting would be silently inert.
///
/// Formatting also keeps the emitter honest for inputs it has never seen. The
/// drift this replaced was entirely width-driven (single-line `if let` bodies,
/// long builder chains, wide struct patterns), so hand-tuning the templates
/// would have held only until a manifest entry arrived with a longer name.
fn generate_cli_rs() -> String {
    rustfmt_source(generate_cli_rs_unformatted())
}

fn generate_cli_rs_unformatted() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by crates/lucidos-engine/src/capability_manifest/codegen.rs — do not edit by hand.\n");
    out.push_str("// Regenerate: cargo test -p lucidos-engine --lib generate_cli_commands_file -- --ignored\n");
    out.push_str("//\n");
    out.push_str(
        "// Source of truth: the capability parity manifest (`capability_manifest::DOMAINS`).\n",
    );
    out.push_str(
        "// Each `cli = true` domain becomes one `clap::Subcommand` enum + one gateway-safe\n",
    );
    out.push_str(
        "// `dispatch_<domain>` that routes through `crate::http` (port/cert/auth handled).\n",
    );
    out.push_str("// Wire each enum into `main.rs`'s `Command` enum + its `run()` match arm.\n");
    out.push_str("//\n");
    out.push_str("// Both allowed lints are inherent to the emitter, not fixable in this file:\n");
    out.push_str(
        "// variant names come verbatim from the manifest's operation names (a domain's\n",
    );
    out.push_str("// variants therefore share prefixes), and the query vector is emitted as\n");
    out.push_str(
        "// `Vec::new()` + one `push` per argument because most arguments are optional.\n",
    );
    out.push_str("#![allow(clippy::enum_variant_names, clippy::vec_init_then_push)]\n\n");
    out.push_str("use crate::http::{client, send_and_print};\n");
    out.push_str("use crate::workspace::{BoxError, Workspace};\n\n");

    for d in domains().iter().filter(|d| d.cli) {
        let cli_ops: Vec<&Operation> = d.operations.iter().filter(|o| o.on_cli(d)).collect();
        let enum_name = format!("{}Cmd", pascal(d.name));
        // Subcommand enum.
        out.push_str(&format!("/// {}\n", d.tool_summary.replace('\n', " ")));
        out.push_str("#[derive(clap::Subcommand)]\n");
        out.push_str(&format!("pub enum {} {{\n", enum_name));
        for op in &cli_ops {
            out.push_str(&format!("    /// {}\n", op.summary));
            let variant = pascal(op.cli_name);
            if op.args.is_empty() {
                out.push_str(&format!("    {},\n", variant));
            } else {
                out.push_str(&format!("    {} {{\n", variant));
                for a in op.args {
                    out.push_str(&format!("        /// {}\n", a.description));
                    out.push_str(&format!(
                        "        #[arg(long)]\n        {}: {},\n",
                        a.name,
                        rust_field_type(a)
                    ));
                }
                out.push_str("    },\n");
            }
        }
        out.push_str("}\n\n");

        // Dispatch fn. The doc comment shows the real (kebab-case) CLI command
        // name (`trigger-groups`), while the fn ident keeps the snake form.
        out.push_str(&format!(
            "/// Execute a `lucidos {} <op>` command against the parent workspace.\n",
            d.name.replace('_', "-")
        ));
        out.push_str(&format!(
            "pub fn dispatch_{}(ws: &Workspace, cmd: {}) -> Result<(), BoxError> {{\n",
            d.name, enum_name
        ));
        out.push_str("    match cmd {\n");
        for op in &cli_ops {
            let variant = pascal(op.cli_name);
            let bind = if op.args.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = op.args.iter().map(|a| a.name).collect();
                format!(" {{ {} }}", names.join(", "))
            };
            out.push_str(&format!(
                "        {}::{}{} => {{\n",
                enum_name, variant, bind
            ));
            // URL (substitute :path args into the format string).
            let path_args: Vec<&Arg> = op.args.iter().filter(|a| a.loc == ArgIn::Path).collect();
            if path_args.is_empty() {
                out.push_str(&format!(
                    "            let url = format!(\"{{}}/api/v1{}\", ws.base_url());\n",
                    op.path
                ));
            } else {
                let mut templ = op.path.to_string();
                let mut fmt_args = vec!["ws.base_url()".to_string()];
                for a in &path_args {
                    templ = templ.replace(&format!(":{}", a.name), "{}");
                    fmt_args.push(a.name.to_string());
                }
                out.push_str(&format!(
                    "            let url = format!(\"{{}}/api/v1{}\", {});\n",
                    templ,
                    fmt_args.join(", ")
                ));
            }
            // Query params — only declare the vec when there are query args.
            let query_args: Vec<&Arg> = op.args.iter().filter(|a| a.loc == ArgIn::Query).collect();
            let query_clause = if query_args.is_empty() {
                String::new()
            } else {
                out.push_str("            let mut query: Vec<(&str, String)> = Vec::new();\n");
                for a in &query_args {
                    match (a.ty, a.required) {
                        // Required bool = bare flag (`bool` field).
                        (ArgType::Bool, true) => out.push_str(&format!(
                            "            if {0} {{ query.push((\"{0}\", \"true\".to_string())); }}\n",
                            a.name
                        )),
                        (_, true) => out.push_str(&format!(
                            "            query.push((\"{0}\", {0}.to_string()));\n",
                            a.name
                        )),
                        // Every optional arg is an `Option<T>`, bool included,
                        // so one arm covers them all.
                        (_, false) => out.push_str(&format!(
                            "            if let Some(v) = {0} {{ query.push((\"{0}\", v.to_string())); }}\n",
                            a.name
                        )),
                    }
                }
                ".query(&query)".to_string()
            };
            // Body — only when there are body args; empty body for non-GET goes
            // through json!({}) so the engine sees a JSON content type.
            let body_args: Vec<&Arg> = op.args.iter().filter(|a| a.loc == ArgIn::Body).collect();
            let body_clause = if !body_args.is_empty() {
                out.push_str("            let mut body = serde_json::Map::new();\n");
                for a in &body_args {
                    // The field is snake_case and clap derives a kebab-case
                    // flag from it, so an error naming the field names a flag
                    // clap then refuses. Say what the user typed.
                    let flag = a.name.replace('_', "-");
                    match (a.ty, a.required) {
                        // Json args arrive as a CLI string; parse them into a
                        // Value so they ride the body as real JSON (not a string).
                        (ArgType::Json, true) => out.push_str(&format!(
                            "            body.insert(\"{0}\".into(), serde_json::from_str::<serde_json::Value>(&{0}).map_err(|e| format!(\"--{1} must be valid JSON: {{}}\", e))?);\n",
                            a.name, flag
                        )),
                        (ArgType::Json, false) => out.push_str(&format!(
                            "            if let Some(v) = {0} {{ body.insert(\"{0}\".into(), serde_json::from_str::<serde_json::Value>(&v).map_err(|e| format!(\"--{1} must be valid JSON: {{}}\", e))?); }}\n",
                            a.name, flag
                        )),
                        (_, true) => out.push_str(&format!(
                            "            body.insert(\"{0}\".into(), serde_json::json!({0}));\n",
                            a.name
                        )),
                        (_, false) => out.push_str(&format!(
                            "            if let Some(v) = {0} {{ body.insert(\"{0}\".into(), serde_json::json!(v)); }}\n",
                            a.name
                        )),
                    }
                }
                ".json(&serde_json::Value::Object(body))".to_string()
            } else if op.method != Method::Get {
                ".json(&serde_json::json!({}))".to_string()
            } else {
                String::new()
            };
            // Build + send.
            let method_call = match op.method {
                Method::Get => "get",
                Method::Post => "post",
                Method::Put => "put",
                Method::Delete => "delete",
            };
            out.push_str(&format!(
                "            let req = client()?.{}(&url){}{};\n",
                method_call, query_clause, body_clause
            ));
            out.push_str(&format!(
                "            send_and_print(\"{}\", &url, req)\n",
                op.method.as_str()
            ));
            out.push_str("        }\n");
        }
        out.push_str("    }\n");
        out.push_str("}\n\n");
    }

    out
}

// ---------------------------------------------------------------------------
// SDK capability table (the parity check target)
// ---------------------------------------------------------------------------

fn generate_sdk_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by crates/lucidos-engine/src/capability_manifest/codegen.rs — do not edit by hand.\n");
    out.push_str("// Regenerate: cargo test -p lucidos-engine --lib generate_sdk_capabilities_file -- --ignored\n");
    out.push_str("//\n");
    out.push_str(
        "// Source of truth: the capability parity manifest. The hand-written SDK facade\n",
    );
    out.push_str(
        "// (e.g. notifications.ts) must expose every `sdkName` listed here for its domain,\n",
    );
    out.push_str(
        "// enforced by capabilities.test.ts so the SDK can't drift behind the manifest.\n\n",
    );
    out.push_str("export interface CapabilityOp {\n");
    out.push_str("  action: string;\n  sdkName: string;\n  method: string;\n  path: string;\n}\n");
    out.push_str("export interface CapabilityDomain {\n");
    out.push_str("  name: string;\n  ops: CapabilityOp[];\n}\n\n");
    out.push_str("export const CAPABILITIES: CapabilityDomain[] = [\n");
    for d in domains().iter().filter(|d| d.sdk) {
        out.push_str("  {\n");
        out.push_str(&format!("    name: '{}',\n", d.name));
        out.push_str("    ops: [\n");
        for op in d.operations.iter().filter(|o| o.on_sdk(d)) {
            out.push_str(&format!(
                "      {{ action: '{}', sdkName: '{}', method: '{}', path: '{}' }},\n",
                op.action,
                op.sdk_name,
                op.method.as_str(),
                op.path
            ));
        }
        out.push_str("    ],\n");
        out.push_str("  },\n");
    }
    out.push_str("];\n");
    out
}

// ---------------------------------------------------------------------------
// Tests: staleness guards + #[ignore] writers
// ---------------------------------------------------------------------------

fn assert_up_to_date(path: std::path::PathBuf, generated: String, regen_cmd: &str) {
    match std::fs::read_to_string(&path) {
        Ok(existing) => assert_eq!(
            existing,
            generated,
            "Generated {} is stale. Run: {}",
            path.display(),
            regen_cmd
        ),
        Err(_) => panic!(
            "Generated file missing at {}. Run: {}",
            path.display(),
            regen_cmd
        ),
    }
}

#[test]
fn generated_cli_commands_is_up_to_date() {
    assert_up_to_date(
        cli_generated_path(),
        generate_cli_rs(),
        "cargo test -p lucidos-engine --lib generate_cli_commands_file -- --ignored",
    );
}

#[test]
fn generated_sdk_capabilities_is_up_to_date() {
    assert_up_to_date(
        sdk_generated_path(),
        generate_sdk_ts(),
        "cargo test -p lucidos-engine --lib generate_sdk_capabilities_file -- --ignored",
    );
}

#[test]
#[ignore]
fn generate_cli_commands_file() {
    let path = cli_generated_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_cli_rs()).unwrap();
    crate::log!("[Codegen] wrote {}", path.display());
}

#[test]
#[ignore]
fn generate_sdk_capabilities_file() {
    let path = sdk_generated_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_sdk_ts()).unwrap();
    crate::log!("[Codegen] wrote {}", path.display());
}

#[test]
fn pascal_handles_kebab_and_snake() {
    assert_eq!(pascal("read-all"), "ReadAll");
    assert_eq!(pascal("mark_all_read"), "MarkAllRead");
    assert_eq!(pascal("list"), "List");
}

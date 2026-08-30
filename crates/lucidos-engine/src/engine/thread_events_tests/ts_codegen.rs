//! Generates the frontend's wire types from the Rust event source.
//!
//! Rust is the source of truth for event types. Only event NAMES used to reach
//! TypeScript by generation, so a new Rust variant was loud and a new Rust
//! FIELD was silent.
//!
//! **The wire shape is not the enum alone.** It is:
//!
//! ```text
//! ThreadEvent variant  +  EventMeta fields  +  API stamps  -  API strips
//! ```
//!
//! `EventMeta::apply` merges `request_event_id`, `channel` and `actor` into
//! every payload. The snapshot endpoint (`api/threads/events_snapshot.rs`) then
//! drops two big fields and stamps two markers. Both halves are declared below,
//! beside the retired variants old DB rows still carry.
//!
//! View models are NOT generated. `store/types.ts` types such as
//! `ContextCapture` add frontend-only fields on purpose, and they consume these
//! generated types rather than re-spelling them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Every named type a wire payload can reach, and the file that declares it.
///
/// This is the forcing function. [`ts_type`] refuses a type that is not here.
/// A payload field naming a new supporting type therefore fails the generator,
/// with that type's name in the message.
const TYPE_SOURCES: &[(&str, &str)] = &[
    ("ActorMode", "engine/thread_events/actor.rs"),
    ("AgentParticipant", "engine/thread_events/actor.rs"),
    ("EngineReason", "engine/thread_events/actor.rs"),
    ("MessageOrigin", "engine/thread_events/actor.rs"),
    ("ThreadDirection", "engine/thread_events/actor.rs"),
    ("AbortCause", "engine/thread_events/cause.rs"),
    ("CancelCause", "engine/thread_events/cause.rs"),
    ("EventWaitCancelCause", "engine/thread_events/cause.rs"),
    ("EventChannel", "engine/thread_events/channel.rs"),
    ("TriggerInvocation", "engine/thread_events/channel.rs"),
    ("AnswerKind", "engine/thread_events/question.rs"),
    ("QuestionOption", "engine/thread_events/question.rs"),
    ("TodoItem", "engine/thread_events/todo.rs"),
    ("TodoStatus", "engine/thread_events/todo.rs"),
    ("ChildCompletionStatus", "engine/thread_events/session.rs"),
    ("SessionEndReason", "engine/thread_events/session.rs"),
    ("VoiceSessionEndReason", "engine/thread_events/session.rs"),
    ("ApiUsage", "engine/types.rs"),
    ("ContextProducer", "engine/types.rs"),
    ("ContextPurpose", "engine/types.rs"),
    ("ContextRole", "engine/types.rs"),
    ("ContextSection", "engine/types.rs"),
    ("ModalityUsage", "engine/types.rs"),
    (
        "CodingAgentKind",
        "engine/agent_session/coding_agent_kind.rs",
    ),
    ("AllowScope", "engine/claude_code/mod.rs"),
    ("EventSubscription", "core/event_subscription/mod.rs"),
    ("CodingAgent", "runtime/agent_runtime.rs"),
];

/// Types the generated file IMPORTS instead of declaring.
///
/// `thread-lifecycle.ts` already emits these two from the same Rust enums, and
/// two declarations of one type is the drift this file exists to end.
const IMPORTED_TYPES: &[(&str, &str)] = &[
    ("EventChannel", "./thread-lifecycle"),
    ("SessionEndReason", "./thread-lifecycle"),
];

const EVENT_SOURCE: &str = "engine/thread_events/event.rs";
const EVENT_IMPL_SOURCE: &str = "engine/thread_events/event_impl.rs";

// ---------------------------------------------------------------------------
// Declared divergences between the Rust type and the wire
// ---------------------------------------------------------------------------

/// Cross-cutting fields `EventMeta::apply` merges into every payload.
///
/// Inlined per member rather than intersected onto the union, so narrowing on
/// `type` keeps working. A variant that declares one of these itself keeps its
/// own (the four change events predate the `EventMeta` path).
const META_FIELDS: &[(&str, &str, &str)] = &[
    (
        "request_event_id",
        "string",
        "Links this event back to the request that opened the turn.",
    ),
    (
        "channel",
        "EventChannel",
        "Source channel. Always set on an origin event.",
    ),
    (
        "actor",
        "MessageOrigin",
        "Who initiated. Absent when an internal state machine acted.",
    ),
];

/// Fields the snapshot endpoint DROPS before the client sees them.
///
/// The wire type has to call them optional even though Rust requires them. See
/// `strip_context_capture_sections`, `strip_tool_result_content` and
/// `strip_tool_call_args` in `api/threads/events_snapshot.rs`. A live SSE
/// emission carries the full value, and a lazy fetch covers the snapshot case.
///
/// The coding-agent rows are the heaviest a workspace holds, and for months
/// the strips knew only the chat channel's names. `CodingAgentToolCalled.args`
/// alone was 606 kB of one reported thread's 1835 kB.
const STRIPPED_FIELDS: &[(&str, &str)] = &[
    ("ContextCaptured", "sections"),
    ("ContextCaptured", "tools"),
    ("ToolResult", "result"),
    ("CodingAgentToolResult", "result"),
    ("CodingAgentToolCalled", "args"),
];

/// Markers the snapshot endpoint STAMPS on, which no Rust variant declares.
const STAMPED_FIELDS: &[(&str, &str, &str)] = &[
    (
        "ContextCaptured",
        "sections_stripped",
        "Server dropped `sections` and `tools` here. The modal lazy-fetches them.",
    ),
    (
        "ToolResult",
        "result_stripped",
        "Server dropped `result` here. The step detail lazy-fetches it.",
    ),
    (
        "CodingAgentToolResult",
        "result_stripped",
        "Server dropped `result` here. The step detail lazy-fetches it.",
    ),
    (
        "CodingAgentToolCalled",
        "args_stripped",
        "Server dropped `args` here. The step detail lazy-fetches them. \
         `description` is filled from `describe_cc_tool` first, so the inline \
         step label never waits on that fetch.",
    ),
];

/// Fields only OLD rows carry. Rust dropped them and the projection still reads
/// them, so they can be neither generated nor deleted.
const LEGACY_FIELDS: &[(&str, &str, &str, &str)] = &[
    (
        "ThoughtStreamed",
        "context_tokens",
        "number",
        "Legacy row only. Superseded by `ContextCaptured`.",
    ),
    (
        "ThoughtStreamed",
        "context_messages",
        "number",
        "Legacy row only. Superseded by `ContextCaptured`.",
    ),
    (
        "ThoughtStreamed",
        "trimmed",
        "boolean",
        "Legacy row only. Superseded by `ContextCaptured`.",
    ),
];

/// Arms the Rust enum no longer has, which old DB rows still carry.
///
/// A serde alias reads an old NAME into the current arm. But the snapshot
/// endpoint serves the raw JSONB column, so the alias never runs and the old
/// spelling reaches the frontend as written. Containment is asserted one way
/// only for that reason (see `thread-event-union.test.ts`).
const LEGACY_VARIANTS: &[LegacyVariant] = &[
    LegacyVariant {
        owner: "EngineReason",
        name: "session_recovered",
        doc: "The pre-rename name for `continuation_started`, carried by rows written before the rename.",
        fields: &[],
    },
    LegacyVariant {
        owner: "ThreadEvent",
        name: "ContextTokensMeasured",
        doc: "Retired. The pre-`ContextCaptured` token measurement.",
        fields: &[("input_tokens", "number")],
    },
    LegacyVariant {
        owner: "ThreadEvent",
        name: "ContextAssembled",
        doc: "Retired. The pre-`ContextCaptured` context snapshot.",
        fields: &[
            ("sections", "ContextSection[]"),
            ("tools", "string[]"),
            ("model", "string"),
            ("total_chars", "number"),
        ],
    },
    LegacyVariant {
        owner: "ThreadEvent",
        name: "MemorySearched",
        doc: "The pre-rename name for `MemoryRecalled`. The snapshot endpoint serves the raw `event_type` column, so the Rust serde alias never reaches us.",
        fields: &[("results?", "number"), ("queries?", "string[]")],
    },
];

/// One union member the Rust enum no longer has, spelled out for the emitter.
struct LegacyVariant {
    /// The type that carries it: `ThreadEvent`, or a supporting tagged enum.
    owner: &'static str,
    name: &'static str,
    doc: &'static str,
    /// Field name and TypeScript type. A trailing `?` marks it optional.
    fields: &'static [(&'static str, &'static str)],
}

/// Fields whose wire type is narrower than the Rust type.
///
/// `condition::validate` refuses anything but an object at the write surface
/// and `evaluate` answers false for one, so `Option<Value>` is an object in
/// practice. The trigger form and the SDK both say so too.
const FIELD_TYPE_OVERRIDES: &[(&str, &str, &str)] = &[
    ("EventSubscription", "condition", "Record<string, unknown>"),
    // The coding agent's tool arguments are always a JSON object.
    (
        "CodingAgentPermissionRequest",
        "input",
        "Record<string, unknown>",
    ),
    // Widened because the snapshot serves the raw column: a row written
    // before the Phase 4 reasons still carries a retired string, which no
    // serde alias rewrites on the way out.
    ("SessionEnded", "reason", "SessionEndReason | string"),
];

/// Longest doc paragraph carried into the generated file.
///
/// `scripts/lib/prose_scan.sh` caps a comment block at 20 lines, delimiters
/// included, and the generated file is scanned like any other source. Only the
/// first paragraph is carried; the rest stays in the Rust source.
const MAX_DOC_LINES: usize = 18;

const DOC_TRUNCATED_NOTE: &str = "Full reasoning is on the Rust variant.";

// ---------------------------------------------------------------------------
// Intermediate representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
struct Doc(Vec<String>);

impl Doc {
    /// The first paragraph, capped. The bool says whether anything was cut.
    fn first_paragraph(&self) -> (Vec<String>, bool) {
        let para: Vec<String> = self
            .0
            .iter()
            .take_while(|l| !l.is_empty())
            .cloned()
            .collect();
        let cut = para.len() < self.0.len() || para.len() > MAX_DOC_LINES;
        (para.into_iter().take(MAX_DOC_LINES).collect(), cut)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    name: String,
    ts_type: String,
    optional: bool,
    doc: Doc,
}

#[derive(Debug, Clone, PartialEq)]
struct Variant {
    name: String,
    fields: Vec<Field>,
    doc: Doc,
}

#[derive(Debug, Clone, PartialEq)]
enum TypeDef {
    /// A serde unit enum. Becomes a union of string literals.
    StringUnion { values: Vec<String>, doc: Doc },
    /// An internally tagged enum. Becomes a discriminated union.
    TaggedUnion {
        tag: String,
        variants: Vec<Variant>,
        doc: Doc,
    },
    /// A struct. Becomes an interface.
    Interface { fields: Vec<Field>, doc: Doc },
}

/// Everything parsed out of the Rust source, ready to emit.
struct Ir {
    /// Supporting types, keyed by Rust name.
    types: BTreeMap<String, TypeDef>,
    /// `ThreadEvent` variants, in declaration order.
    variants: Vec<Variant>,
    /// Names `is_persisted` reports as transient.
    transient: BTreeSet<String>,
}

// ---------------------------------------------------------------------------
// Serde attribute reading
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SerdeAttrs {
    rename: Option<String>,
    rename_all: Option<String>,
    tag: Option<String>,
    has_default: bool,
    skip_serializing_if: bool,
    skip: bool,
    flatten: bool,
}

fn serde_attrs(attrs: &[syn::Attribute]) -> SerdeAttrs {
    let mut out = SerdeAttrs::default();
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        // A parse error here can only mean an attribute shape this reader does
        // not model. It cannot silently change the wire, and the emitted file
        // is diffed against the checked-in one, so a surprise still shows up.
        let _ = attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let value = || {
                meta.value()
                    .ok()
                    .and_then(|v| v.parse::<syn::LitStr>().ok())
                    .map(|s| s.value())
            };
            match key.as_str() {
                "rename" => out.rename = value(),
                "rename_all" => out.rename_all = value(),
                "tag" => out.tag = value(),
                "default" => {
                    out.has_default = true;
                    value();
                }
                "skip_serializing_if" => {
                    out.skip_serializing_if = true;
                    value();
                }
                "skip" => out.skip = true,
                "flatten" => out.flatten = true,
                _ => {
                    value();
                }
            }
            Ok(())
        });
    }
    out
}

fn doc_of(attrs: &[syn::Attribute]) -> Doc {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                lines.push(s.value().trim().to_string());
            }
        }
    }
    Doc(lines)
}

fn to_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn apply_rename_all(name: &str, rule: Option<&str>) -> Result<String, String> {
    let Some(rule) = rule else {
        return Ok(name.to_string());
    };
    match rule {
        "PascalCase" => Ok(name.to_string()),
        "lowercase" => Ok(name.to_lowercase()),
        "UPPERCASE" => Ok(name.to_uppercase()),
        "snake_case" => Ok(to_snake(name)),
        "kebab-case" => Ok(to_snake(name).replace('_', "-")),
        "SCREAMING_SNAKE_CASE" => Ok(to_snake(name).to_uppercase()),
        other => Err(format!(
            "unsupported serde rename_all rule `{other}`; teach `apply_rename_all` about it"
        )),
    }
}

// ---------------------------------------------------------------------------
// Rust type to TypeScript type
// ---------------------------------------------------------------------------

fn is_registered(name: &str) -> bool {
    TYPE_SOURCES.iter().any(|(n, _)| *n == name)
}

/// Map a Rust type onto its wire spelling.
///
/// `Option` is peeled here. A caller decides optionality from the serde
/// attributes instead, because the wire omits the key rather than sending null.
fn ts_type(ty: &syn::Type) -> Result<String, String> {
    let syn::Type::Path(path) = ty else {
        return Err("only a path type can appear in an event payload".into());
    };
    let seg = path
        .path
        .segments
        .last()
        .ok_or_else(|| "empty type path".to_string())?;
    let name = seg.ident.to_string();
    let args: Vec<&syn::Type> = match &seg.arguments {
        syn::PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|a| match a {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let arg = |i: usize| -> Result<&syn::Type, String> {
        args.get(i)
            .copied()
            .ok_or_else(|| format!("`{name}` needs a type argument at position {i}"))
    };
    match name.as_str() {
        "Option" => ts_type(arg(0)?),
        "Vec" => Ok(format!("{}[]", ts_type(arg(0)?)?)),
        "HashMap" | "BTreeMap" => Ok(format!("Record<string, {}>", ts_type(arg(1)?)?)),
        "String" | "str" => Ok("string".into()),
        "bool" => Ok("boolean".into()),
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => Ok("number".into()),
        // Both serialize to a string, so the frontend reads them as one.
        "Uuid" | "DateTime" => Ok("string".into()),
        "Value" => Ok("unknown".into()),
        other if is_registered(other) => Ok(other.to_string()),
        other => Err(format!(
            "`{other}` reaches the wire but is not in TYPE_SOURCES. Register it with the \
             file that declares it, or the frontend cannot be told what it is"
        )),
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn parse_source(rel: &str) -> syn::File {
    let path = crate_src().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    syn::parse_file(&text).unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

fn parse_fields(
    owner: &str,
    fields: &syn::Fields,
    container_rename_all: Option<&str>,
) -> Result<Vec<Field>, String> {
    let syn::Fields::Named(named) = fields else {
        if matches!(fields, syn::Fields::Unit) {
            return Ok(Vec::new());
        }
        return Err(format!(
            "`{owner}` uses a tuple shape; the wire needs named fields"
        ));
    };
    let mut out = Vec::new();
    for f in &named.named {
        let attrs = serde_attrs(&f.attrs);
        if attrs.skip {
            continue;
        }
        if attrs.flatten {
            return Err(format!(
                "`{owner}` flattens a field; the emitter cannot inline it"
            ));
        }
        let ident = f
            .ident
            .as_ref()
            .ok_or_else(|| format!("`{owner}` has an unnamed field"))?
            .to_string();
        let name = match &attrs.rename {
            Some(r) => r.clone(),
            None => apply_rename_all(&ident, container_rename_all)?,
        };
        let ts = match FIELD_TYPE_OVERRIDES
            .iter()
            .find(|(o, fname, _)| *o == owner && *fname == name)
        {
            Some((_, _, over)) => (*over).to_string(),
            None => ts_type(&f.ty).map_err(|e| format!("{owner}.{name}: {e}"))?,
        };
        let stripped = STRIPPED_FIELDS
            .iter()
            .any(|(v, fname)| *v == owner && *fname == name);
        out.push(Field {
            name,
            ts_type: ts,
            // The wire omits a key whenever serde may skip it, and an old row
            // predates any field carrying a `default`. Both read as absent.
            optional: attrs.has_default || attrs.skip_serializing_if || stripped,
            doc: doc_of(&f.attrs),
        });
    }
    Ok(out)
}

fn parse_enum(item: &syn::ItemEnum) -> Result<TypeDef, String> {
    let container = serde_attrs(&item.attrs);
    let rename_all = container.rename_all.as_deref();
    let doc = doc_of(&item.attrs);
    let name = item.ident.to_string();
    let all_unit = item
        .variants
        .iter()
        .all(|v| matches!(v.fields, syn::Fields::Unit));
    if all_unit && container.tag.is_none() {
        let mut values = Vec::new();
        for v in &item.variants {
            let attrs = serde_attrs(&v.attrs);
            values.push(match &attrs.rename {
                Some(r) => r.clone(),
                None => apply_rename_all(&v.ident.to_string(), rename_all)?,
            });
        }
        return Ok(TypeDef::StringUnion { values, doc });
    }
    let tag = container.tag.ok_or_else(|| {
        format!("`{name}` carries data but is not internally tagged; the frontend cannot narrow it")
    })?;
    let mut variants = Vec::new();
    for v in &item.variants {
        let attrs = serde_attrs(&v.attrs);
        let wire = match &attrs.rename {
            Some(r) => r.clone(),
            None => apply_rename_all(&v.ident.to_string(), rename_all)?,
        };
        variants.push(Variant {
            fields: parse_fields(&format!("{name}::{wire}"), &v.fields, rename_all)?,
            name: wire,
            doc: doc_of(&v.attrs),
        });
    }
    Ok(TypeDef::TaggedUnion { tag, variants, doc })
}

fn parse_named_type(name: &str, rel: &str) -> Result<TypeDef, String> {
    let file = parse_source(rel);
    for item in &file.items {
        match item {
            syn::Item::Enum(e) if e.ident == name => return parse_enum(e),
            syn::Item::Struct(s) if s.ident == name => {
                let container = serde_attrs(&s.attrs);
                return Ok(TypeDef::Interface {
                    fields: parse_fields(name, &s.fields, container.rename_all.as_deref())?,
                    doc: doc_of(&s.attrs),
                });
            }
            _ => {}
        }
    }
    Err(format!(
        "`{name}` is registered against {rel} but is not declared there"
    ))
}

/// Variant names `ThreadEvent::is_persisted` reports as transient.
///
/// Read from the source rather than restated, so the union split and the
/// runtime predicate cannot disagree. The function is one `!matches!`, and a
/// reshaping this cannot read fails the generator by name.
fn parse_transient_names() -> BTreeSet<String> {
    let file = parse_source(EVENT_IMPL_SOURCE);
    for item in &file.items {
        let syn::Item::Impl(imp) = item else { continue };
        for it in &imp.items {
            let syn::ImplItem::Fn(f) = it else { continue };
            if f.sig.ident != "is_persisted" {
                continue;
            }
            return f
                .block
                .stmts
                .iter()
                .find_map(|s| match s {
                    syn::Stmt::Expr(e, _) => macro_self_paths(e),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("`is_persisted` no longer wraps one `matches!` over `Self::` paths")
                });
        }
    }
    panic!("`is_persisted` not found in {EVENT_IMPL_SOURCE}");
}

/// Peel wrappers off an expression, then collect the `Self::X` names its macro
/// arms mention.
fn macro_self_paths(expr: &syn::Expr) -> Option<BTreeSet<String>> {
    match expr {
        syn::Expr::Unary(u) => macro_self_paths(&u.expr),
        syn::Expr::Paren(p) => macro_self_paths(&p.expr),
        syn::Expr::Group(g) => macro_self_paths(&g.expr),
        syn::Expr::Macro(m) => {
            let tokens: Vec<proc_macro2::TokenTree> = m.mac.tokens.clone().into_iter().collect();
            let mut out = BTreeSet::new();
            for w in tokens.windows(4) {
                let is_self = matches!(&w[0], proc_macro2::TokenTree::Ident(i) if *i == "Self");
                let colons = matches!(&w[1], proc_macro2::TokenTree::Punct(p) if p.as_char() == ':')
                    && matches!(&w[2], proc_macro2::TokenTree::Punct(p) if p.as_char() == ':');
                if is_self && colons {
                    if let proc_macro2::TokenTree::Ident(i) = &w[3] {
                        out.insert(i.to_string());
                    }
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

fn load_ir() -> Ir {
    let mut types = BTreeMap::new();
    for (name, rel) in TYPE_SOURCES {
        let def = parse_named_type(name, rel).unwrap_or_else(|e| panic!("{e}"));
        types.insert((*name).to_string(), def);
    }

    let file = parse_source(EVENT_SOURCE);
    let item = file
        .items
        .iter()
        .find_map(|i| match i {
            syn::Item::Enum(e) if e.ident == "ThreadEvent" => Some(e),
            _ => None,
        })
        .expect("`ThreadEvent` not found in event.rs");
    let container = serde_attrs(&item.attrs);
    assert_eq!(
        container.tag.as_deref(),
        Some("type"),
        "`ThreadEvent` must stay internally tagged on `type`; the frontend narrows on it"
    );
    let rename_all = container.rename_all.as_deref();
    let mut variants = Vec::new();
    for v in &item.variants {
        let attrs = serde_attrs(&v.attrs);
        let name = match &attrs.rename {
            Some(r) => r.clone(),
            None => apply_rename_all(&v.ident.to_string(), rename_all).unwrap(),
        };
        let fields = parse_fields(&name, &v.fields, rename_all).unwrap_or_else(|e| panic!("{e}"));
        variants.push(Variant {
            name,
            fields,
            doc: doc_of(&v.attrs),
        });
    }

    Ir {
        types,
        variants,
        transient: parse_transient_names(),
    }
}

// ---------------------------------------------------------------------------
// Carried prose has to obey the tree's own rules
// ---------------------------------------------------------------------------

/// True for `YYYY-MM-DD` anywhere in the line, matching `prose_scan.sh`.
fn has_iso_date(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.windows(10).any(|w| {
        w[0] == '2'
            && w[1] == '0'
            && w[2..4].iter().all(char::is_ascii_digit)
            && w[4] == '-'
            && w[5..7].iter().all(char::is_ascii_digit)
            && w[7] == '-'
            && w[8..10].iter().all(char::is_ascii_digit)
    })
}

/// Reasons this doc cannot be copied into the generated file.
///
/// The generated file ships, and it is scanned like any hand-written source.
/// So a carried em dash or ISO date fails a gate on a file nobody wrote.
/// Refusing here points the author at the Rust line, where the rule binds
/// anyway.
fn doc_faults(owner: &str, doc: &Doc) -> Vec<String> {
    let (lines, _) = doc.first_paragraph();
    let mut out = Vec::new();
    for l in &lines {
        if l.contains('\u{2014}') || l.contains('\u{2015}') {
            out.push(format!(
                "{owner}: the doc comment carries an em dash or horizontal bar. Rewrite the \
                 Rust line with a comma, a colon, parentheses or two sentences: {l}"
            ));
        }
        if has_iso_date(l) {
            out.push(format!(
                "{owner}: the doc comment carries an ISO date. Git and docs/plans hold \
                 history; rewrite the Rust line: {l}"
            ));
        }
    }
    out
}

fn field_faults(owner: &str, fields: &[Field]) -> Vec<String> {
    fields
        .iter()
        .flat_map(|f| doc_faults(&format!("{owner}.{}", f.name), &f.doc))
        .collect()
}

/// Every carried doc the generator refuses, collected so one run names them all.
fn all_doc_faults(ir: &Ir) -> Vec<String> {
    let mut out = Vec::new();
    for (name, def) in &ir.types {
        match def {
            TypeDef::StringUnion { doc, .. } => out.extend(doc_faults(name, doc)),
            TypeDef::Interface { fields, doc } => {
                out.extend(doc_faults(name, doc));
                out.extend(field_faults(name, fields));
            }
            TypeDef::TaggedUnion { variants, doc, .. } => {
                out.extend(doc_faults(name, doc));
                for v in variants {
                    let owner = format!("{name}::{}", v.name);
                    out.extend(doc_faults(&owner, &v.doc));
                    out.extend(field_faults(&owner, &v.fields));
                }
            }
        }
    }
    for v in &ir.variants {
        let owner = format!("ThreadEvent::{}", v.name);
        out.extend(doc_faults(&owner, &v.doc));
        out.extend(field_faults(&owner, &v.fields));
    }
    out
}

// ---------------------------------------------------------------------------
// Emitting
// ---------------------------------------------------------------------------

fn emit_doc(out: &mut String, doc: &Doc, indent: &str) {
    let (mut lines, truncated) = doc.first_paragraph();
    if lines.is_empty() {
        return;
    }
    if truncated {
        lines.push(DOC_TRUNCATED_NOTE.to_string());
    }
    if lines.len() == 1 {
        out.push_str(&format!("{indent}/** {} */\n", lines[0]));
        return;
    }
    out.push_str(&format!("{indent}/** {}\n", lines[0]));
    for l in &lines[1..lines.len() - 1] {
        out.push_str(&format!("{indent} *  {l}\n"));
    }
    out.push_str(&format!("{indent} *  {} */\n", lines[lines.len() - 1]));
}

fn emit_field(out: &mut String, f: &Field, indent: &str) {
    emit_doc(out, &f.doc, indent);
    let q = if f.optional { "?" } else { "" };
    out.push_str(&format!("{indent}{}{q}: {};\n", f.name, f.ts_type));
}

/// Close the union just written: drop the trailing newline, add a semicolon.
fn close_union(out: &mut String) {
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push_str(";\n\n");
}

fn emit_supporting(out: &mut String, name: &str, def: &TypeDef) {
    match def {
        TypeDef::StringUnion { values, doc } => {
            emit_doc(out, doc, "");
            let body: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
            out.push_str(&format!(
                "export type {name} =\n  | {};\n\n",
                body.join("\n  | ")
            ));
        }
        TypeDef::TaggedUnion { tag, variants, doc } => {
            emit_doc(out, doc, "");
            out.push_str(&format!("export type {name} =\n"));
            for v in variants.iter().chain(legacy_variants(name).iter()) {
                emit_doc(out, &v.doc, "  ");
                if v.fields.is_empty() {
                    out.push_str(&format!("  | {{ {tag}: '{}' }}\n", v.name));
                    continue;
                }
                out.push_str(&format!("  | {{\n      {tag}: '{}';\n", v.name));
                for f in &v.fields {
                    emit_field(out, f, "      ");
                }
                out.push_str("    }\n");
            }
            close_union(out);
        }
        TypeDef::Interface { fields, doc } => {
            emit_doc(out, doc, "");
            out.push_str(&format!("export interface {name} {{\n"));
            for f in fields {
                emit_field(out, f, "  ");
            }
            out.push_str("}\n\n");
        }
    }
}

/// Every field of one wire member: the Rust fields, then the API stamps, the
/// legacy fields, and the meta fields a variant does not already declare.
fn wire_fields(variant: &Variant) -> Vec<Field> {
    let mut fields = variant.fields.clone();
    let mut push = |name: &str, ts: &str, doc: &str| {
        if fields.iter().any(|f| f.name == name) {
            return;
        }
        fields.push(Field {
            name: name.to_string(),
            ts_type: ts.to_string(),
            optional: true,
            doc: Doc(vec![doc.to_string()]),
        });
    };
    for (v, name, doc) in STAMPED_FIELDS {
        if *v == variant.name {
            push(name, "boolean", doc);
        }
    }
    for (v, name, ts, doc) in LEGACY_FIELDS {
        if *v == variant.name {
            push(name, ts, doc);
        }
    }
    for (name, ts, doc) in META_FIELDS {
        push(name, ts, doc);
    }
    fields
}

fn emit_union(out: &mut String, name: &str, members: &[Variant]) {
    out.push_str(&format!("export type {name} =\n"));
    for v in members {
        emit_doc(out, &v.doc, "  ");
        let fields = wire_fields(v);
        out.push_str(&format!("  | {{\n      type: '{}';\n", v.name));
        for f in &fields {
            emit_field(out, f, "      ");
        }
        out.push_str("    }\n");
    }
    close_union(out);
}

fn legacy_variants(owner: &str) -> Vec<Variant> {
    LEGACY_VARIANTS
        .iter()
        .filter(|v| v.owner == owner)
        .map(|v| Variant {
            name: v.name.to_string(),
            doc: Doc(vec![v.doc.to_string()]),
            fields: v
                .fields
                .iter()
                .map(|(fname, ts)| Field {
                    name: fname.trim_end_matches('?').to_string(),
                    ts_type: (*ts).to_string(),
                    optional: fname.ends_with('?'),
                    doc: Doc::default(),
                })
                .collect(),
        })
        .collect()
}

fn generate_typescript(ir: &Ir) -> String {
    let faults = all_doc_faults(ir);
    assert!(
        faults.is_empty(),
        "{} Rust doc comment(s) cannot be carried into the generated file:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by thread_events_tests/ts_codegen.rs. Do not edit by hand.\n");
    out.push_str(
        "// Regenerate: cargo test -p lucidos-engine generate_thread_event_wire_file -- --ignored\n",
    );
    out.push_str("//\n");
    out.push_str("// The WIRE shape of a thread event: the Rust `ThreadEvent` variant, plus the\n");
    out.push_str("// `EventMeta` fields the bus merges in, plus the markers the snapshot\n");
    out.push_str("// endpoint stamps, minus the fields it strips. View models live in\n");
    out.push_str("// `store/types.ts` and consume these rather than re-spelling them.\n\n");

    let mut imports: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, module) in IMPORTED_TYPES {
        imports.entry(module).or_default().push(name);
    }
    for (module, names) in &imports {
        out.push_str(&format!(
            "import type {{ {} }} from '{module}';\n",
            names.join(", ")
        ));
    }
    out.push_str(&format!(
        "\nexport type {{ {} }};\n\n",
        IMPORTED_TYPES
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let imported: BTreeSet<&str> = IMPORTED_TYPES.iter().map(|(n, _)| *n).collect();
    for (name, def) in &ir.types {
        if imported.contains(name.as_str()) {
            continue;
        }
        emit_supporting(&mut out, name, def);
    }

    let persisted: Vec<Variant> = ir
        .variants
        .iter()
        .filter(|v| !ir.transient.contains(&v.name))
        .cloned()
        .chain(legacy_variants("ThreadEvent"))
        .collect();
    let transient: Vec<Variant> = ir
        .variants
        .iter()
        .filter(|v| ir.transient.contains(&v.name))
        .cloned()
        .collect();

    emit_union(&mut out, "ThreadEvent", &persisted);
    emit_union(&mut out, "TransientEvent", &transient);

    out.push_str("/** Every `ThreadEvent` discriminant, as a compile-time-checked object. The\n");
    out.push_str(" *  `satisfies` annotation is what makes the runtime set below provably\n");
    out.push_str(" *  exhaustive over the union. */\n");
    out.push_str("const THREAD_EVENT_TYPE_FLAGS = {\n");
    for v in &persisted {
        out.push_str(&format!("  {}: true,\n", v.name));
    }
    out.push_str("} satisfies Record<ThreadEvent['type'], true>;\n\n");
    out.push_str("/** Runtime-enumerable view of the union. TypeScript types are erased, so the\n");
    out.push_str(" *  union-coverage contract test reads this instead. */\n");
    out.push_str(
        "export const THREAD_EVENT_TYPE_NAMES: ReadonlySet<ThreadEvent['type']> = new Set(\n",
    );
    out.push_str("  Object.keys(THREAD_EVENT_TYPE_FLAGS) as ThreadEvent['type'][],\n");
    out.push_str(");\n");
    assert_output_prose_is_clean(&out);
    out
}

/// Last gate before the file is written.
///
/// `doc_faults` covers what is CARRIED from a Rust doc comment. The declared
/// tables above carry prose too, and it lands in the same shipping file. This
/// reads the emitted text, so neither source can smuggle a banned character
/// past the em-dash and prose gates.
fn assert_output_prose_is_clean(out: &str) {
    let mut faults = Vec::new();
    for (i, line) in out.lines().enumerate() {
        let t = line.trim_start();
        if !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')) {
            continue;
        }
        if line.contains('\u{2014}') || line.contains('\u{2015}') {
            faults.push(format!("line {}: em dash or horizontal bar: {line}", i + 1));
        }
        if has_iso_date(line) {
            faults.push(format!("line {}: ISO date: {line}", i + 1));
        }
    }
    assert!(
        faults.is_empty(),
        "the generated file would carry banned prose. Fix the Rust doc comment \
         or the declared table it came from:\n  {}",
        faults.join("\n  ")
    );
}

fn generated_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/thread-event-wire.ts")
}

// ---------------------------------------------------------------------------
// Contract tests
// ---------------------------------------------------------------------------

#[test]
fn generated_thread_event_wire_is_up_to_date() {
    let generated = generate_typescript(&load_ir());
    let path = generated_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} does not exist. Run: cargo test -p lucidos-engine \
             generate_thread_event_wire_file -- --ignored",
            path.display()
        )
    });
    assert_eq!(
        existing, generated,
        "The generated wire types are stale. Run: cargo test -p lucidos-engine \
         generate_thread_event_wire_file -- --ignored"
    );
}

#[test]
#[ignore]
fn generate_thread_event_wire_file() {
    let generated = generate_typescript(&load_ir());
    let path = generated_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &generated).unwrap();
    crate::log!("[ContractTest] Generated: {}", path.display());
}

/// The hardcoded name list is a SECOND hand-maintained hop, and nothing guarded
/// it. A variant added to the enum but missed there never reaches
/// `EVENT_CLASSIFICATION`, so the frontend's drift guard never sees it either.
#[test]
fn all_persisted_event_types_matches_the_enum() {
    let ir = load_ir();
    let parsed: BTreeSet<String> = ir
        .variants
        .iter()
        .filter(|v| !ir.transient.contains(&v.name))
        .map(|v| v.name.clone())
        .collect();
    let listed: BTreeSet<String> = crate::engine::thread_lifecycle::all_persisted_event_types()
        .into_iter()
        .map(str::to_string)
        .collect();
    let missing: Vec<&String> = parsed.difference(&listed).collect();
    let extra: Vec<&String> = listed.difference(&parsed).collect();
    assert!(
        missing.is_empty(),
        "these persisted `ThreadEvent` variants are missing from \
         `all_persisted_event_types()`, so they never reach EVENT_CLASSIFICATION \
         or the frontend's drift guard: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "`all_persisted_event_types()` names events the enum does not have \
         (or names a transient one): {extra:?}"
    );
}

// ---------------------------------------------------------------------------
// Unit tests over the reader
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit {
    use super::*;

    fn variant<'a>(ir: &'a Ir, name: &str) -> &'a Variant {
        ir.variants
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("no `{name}` variant"))
    }

    fn field<'a>(v: &'a Variant, name: &str) -> &'a Field {
        v.fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no `{name}` field on `{}`", v.name))
    }

    /// The one rule the whole generator turns on. The wire omits a key when
    /// serde may skip it, and an old row predates any field carrying a
    /// `default`. Both read as absent, so both are `?`.
    #[test]
    fn optionality_follows_the_serde_attributes() {
        let ir = load_ir();
        let msg = variant(&ir, "MessageReceived");
        assert!(!field(msg, "text").optional, "a bare field is required");
        assert!(
            field(msg, "user_image_hashes").optional,
            "`skip_serializing_if = Vec::is_empty` means the key can be absent"
        );
        assert!(
            field(msg, "mode").optional,
            "`serde(default)` means an old row can lack the key"
        );
        assert!(
            field(msg, "device_id").optional,
            "an Option with a skip predicate is absent, never null"
        );
    }

    /// `Option<T>` is `T`, and the property carries the absence instead.
    #[test]
    fn option_never_becomes_a_nullable_type() {
        let ir = load_ir();
        let msg = variant(&ir, "MessageReceived");
        assert_eq!(field(msg, "device_id").ts_type, "string");
        assert_eq!(field(msg, "origin").ts_type, "MessageOrigin");
        let bg = variant(&ir, "BackgroundBashCompleted");
        assert_eq!(field(bg, "exit_code").ts_type, "number");
    }

    /// A field the snapshot endpoint drops has to be optional on the wire. Rust
    /// requires it, but the lazy-fetch path would read a promise nobody keeps.
    ///
    /// Driven off `STRIPPED_FIELDS` rather than a hand-listed few, so a strip
    /// added to that table cannot arrive uncovered. `variant` and `field` panic
    /// on a name that resolves to nothing, which is the other half: an entry
    /// naming a field no variant has fails here rather than generating quietly.
    #[test]
    fn stripped_fields_are_optional_despite_being_required_in_rust() {
        let ir = load_ir();
        assert!(!STRIPPED_FIELDS.is_empty(), "the table must not be empty");
        for (variant_name, field_name) in STRIPPED_FIELDS {
            let v = variant(&ir, variant_name);
            assert!(
                field(v, field_name).optional,
                "`{variant_name}.{field_name}` is stripped, so the wire must call it optional"
            );
        }
    }

    /// A stamped marker is optional too, and belongs to a variant that exists.
    /// Its own field is absent from Rust by definition, so `wire_fields` is
    /// what has to carry it.
    #[test]
    fn stamped_markers_reach_the_wire_as_optional_fields() {
        let ir = load_ir();
        assert!(!STAMPED_FIELDS.is_empty(), "the table must not be empty");
        for (variant_name, field_name, _) in STAMPED_FIELDS {
            let fields = wire_fields(variant(&ir, variant_name));
            let stamped = fields
                .iter()
                .find(|f| &f.name == field_name)
                .unwrap_or_else(|| panic!("`{variant_name}` never gained `{field_name}`"));
            assert!(
                stamped.optional,
                "a marker is absent unless the server stamped it"
            );
            assert_eq!(stamped.ts_type, "boolean");
        }
    }

    /// Every variant carries the three `EventMeta` fields, including one with
    /// no `actor` of its own.
    #[test]
    fn every_wire_member_carries_the_meta_fields() {
        let ir = load_ir();
        for v in &ir.variants {
            let names: Vec<String> = wire_fields(v).iter().map(|f| f.name.clone()).collect();
            for (meta, _, _) in META_FIELDS {
                assert!(
                    names.contains(&(*meta).to_string()),
                    "`{}` is missing the wire meta field `{meta}`",
                    v.name
                );
            }
        }
        let streamed = wire_fields(variant(&ir, "TextStreamed"));
        let actor = streamed.iter().find(|f| f.name == "actor").unwrap();
        assert!(actor.optional, "a merged meta field is always optional");
    }

    /// A variant that declares `actor` itself keeps its own, rather than
    /// gaining a second copy from the meta merge.
    #[test]
    fn a_variant_with_its_own_actor_is_not_given_a_second_one() {
        let ir = load_ir();
        let applied = wire_fields(variant(&ir, "ChangeApplied"));
        assert_eq!(
            applied.iter().filter(|f| f.name == "actor").count(),
            1,
            "`ChangeApplied` predates the EventMeta path and declares `actor` itself"
        );
    }

    /// Renaming rules produce the wire strings the frontend already reads.
    #[test]
    fn rename_all_rules_match_the_wire() {
        let snake = apply_rename_all("MainLlm", Some("snake_case")).unwrap();
        assert_eq!(snake, "main_llm");
        let kebab = apply_rename_all("ClaudeCode", Some("kebab-case")).unwrap();
        assert_eq!(kebab, "claude-code");
        assert_eq!(
            apply_rename_all("Human", Some("lowercase")).unwrap(),
            "human"
        );
        assert_eq!(apply_rename_all("Selected", None).unwrap(), "Selected");
        assert!(apply_rename_all("X", Some("Train-Case")).is_err());
    }

    /// The transient split is read from `is_persisted`, so the two cannot
    /// disagree about which events the DB keeps.
    #[test]
    fn the_transient_set_comes_from_is_persisted() {
        let ir = load_ir();
        assert!(ir.transient.contains("CumulativeTextUpdated"));
        assert!(ir.transient.contains("ChildrenCountChanged"));
        assert!(!ir.transient.contains("MessageReceived"));
        assert_eq!(
            ir.transient.len(),
            14,
            "the transient list changed; check `is_persisted` and the frontend's \
             `TransientEvent` union together"
        );
    }

    /// An unregistered payload type must fail loudly. It is the only thing
    /// standing between a new supporting type and a frontend that cannot name
    /// it.
    #[test]
    fn an_unregistered_type_fails_the_generator() {
        let ty: syn::Type = syn::parse_str("Option<SomeBrandNewThing>").unwrap();
        let err = ts_type(&ty).unwrap_err();
        assert!(err.contains("SomeBrandNewThing"), "got: {err}");
        assert!(
            err.contains("TYPE_SOURCES"),
            "the message must say where to register it"
        );
    }

    /// The doc carry-across keeps the generated file inside the tree's own
    /// comment-block limit. `scripts/check-prose.sh` cannot then fail on prose
    /// the generator wrote.
    #[test]
    fn carried_docs_stay_inside_the_comment_block_limit() {
        let generated = generate_typescript(&load_ir());
        let mut run = 0usize;
        for (i, line) in generated.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
                run += 1;
                assert!(
                    run <= 20,
                    "comment block runs past 20 lines at line {}",
                    i + 1
                );
            } else {
                run = 0;
            }
        }
    }
}

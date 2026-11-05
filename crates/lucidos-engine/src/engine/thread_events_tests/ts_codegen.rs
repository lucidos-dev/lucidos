use super::*;

fn load_ir() -> Ir {
    super::load_ir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
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
            10,
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
}

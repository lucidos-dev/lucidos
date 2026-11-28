mod guard_tests {
    use super::super::*;

    fn repo_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn load_ir() -> Ir {
        super::super::load_ir(&repo_root().join(SOURCE_ROOT))
    }

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

        #[test]
        fn option_never_becomes_a_nullable_type() {
            let ir = load_ir();
            let msg = variant(&ir, "MessageReceived");
            assert_eq!(field(msg, "device_id").ts_type, "string");
            assert_eq!(field(msg, "origin").ts_type, "MessageOrigin");
            let bg = variant(&ir, "BackgroundBashCompleted");
            assert_eq!(field(bg, "exit_code").ts_type, "number");
        }

        #[test]
        fn spend_cap_notice_keeps_its_required_nullable_reset() {
            let ir = load_ir();
            let definition = ir.types.get("SpendCapNotice").unwrap();
            let TypeDef::Interface { fields, .. } = definition else {
                panic!("SpendCapNotice must generate an interface");
            };
            let reset = fields
                .iter()
                .find(|field| field.name == "resets_at")
                .unwrap();
            assert!(!reset.optional, "serde always emits the reset key");
            assert_eq!(reset.ts_type, "string | null");
            let mut rendered = String::new();
            emit_supporting(&mut rendered, "SpendCapNotice", definition);
            assert!(rendered.contains("  resets_at: string | null;"));
        }

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

        #[test]
        fn the_transient_set_comes_from_is_persisted() {
            let ir = load_ir();
            assert!(ir.transient.contains("CumulativeTextUpdated"));
            assert!(ir.transient.contains("ChildrenCountChanged"));
            assert!(!ir.transient.contains("MessageReceived"));
            assert_eq!(
                ir.transient.len(),
                11,
                "the transient list changed; check `is_persisted` and the frontend's \
             `TransientEvent` union together"
            );
        }

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

    #[test]
    fn externally_tagged_scopes_keep_unit_strings_and_named_payloads() {
        let item: syn::ItemEnum = syn::parse_str(
            r#"#[serde(rename_all = "snake_case")] enum Scope {
                Asked,
                #[serde(rename = "device-session")] Device { ttl_secs: u64 },
                Empty {},
                #[serde(skip)] Hidden { secret: String },
            }"#,
        )
        .unwrap();
        let mut out = String::new();
        emit_supporting(&mut out, "Scope", &parse_enum(&item).unwrap());
        assert!(out.contains("| 'asked'"));
        assert!(out.contains("'device-session': {\n        ttl_secs: number;"));
        assert!(out.contains("'empty': {\n"));
        assert!(!out.contains("Hidden"));
        assert!(!out.contains("secret"));
        for attr in [r#"untagged"#, r#"tag = "kind", content = "data""#] {
            let item = syn::parse_str(&format!(
                "#[serde({attr})] enum E {{ Value {{ field: u8 }} }}"
            ))
            .unwrap();
            assert!(parse_enum(&item).is_err());
        }
        let item =
            syn::parse_str("enum E { Asked, #[serde(untagged)] Custom { pattern: String } }")
                .unwrap();
        assert!(parse_enum(&item).is_err());
    }

    #[test]
    fn a_skipped_variant_never_reaches_the_wire_union() {
        let unit: syn::ItemEnum = syn::parse_str(
            "#[serde(rename_all = \"snake_case\")] enum E { Kept, #[serde(skip)] Hidden }",
        )
        .unwrap();
        let TypeDef::StringUnion { values, .. } = parse_enum(&unit).unwrap() else {
            panic!("a unit enum is a string union");
        };
        assert_eq!(values, ["kept"]);

        let tagged: syn::ItemEnum = syn::parse_str(
            "#[serde(tag = \"kind\")] enum T { Kept { a: u8 }, #[serde(skip)] Hidden { b: u8 } }",
        )
        .unwrap();
        let TypeDef::TaggedUnion { variants, .. } = parse_enum(&tagged).unwrap() else {
            panic!("a tagged enum is a tagged union");
        };
        let names: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Kept"]);
    }

    #[test]
    fn the_header_names_the_core_regenerate_command() {
        let ir = load_ir();
        assert!(!ir.regenerate.is_empty());
        assert!(generate_typescript(&ir).contains(&format!("// Regenerate: {}\n", ir.regenerate)));
    }

    #[test]
    fn the_reader_depends_on_no_workspace_crate() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("the manifest reads");
        for line in manifest.lines().filter(|line| !line.starts_with('#')) {
            assert!(
                !line.contains("lucidos-") || line.starts_with("name = "),
                "the reader must not depend on a workspace crate: {line}"
            );
            assert!(!line.contains("path ="), "no path dependency: {line}");
        }
    }
}

use crate::runtime::agent_sandbox::Sandbox;
use lucidos_agent_sandbox::Policy;

/// A head grant inside an open workspace cannot open a symlink into hidden storage.
#[cfg(unix)]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn workspace_carve_out_cannot_follow_links_before_or_after_a_grant() {
    use crate::engine::command_guard::{self, CommandSite, TrustedHeads};
    let scratch = tempfile::tempdir().unwrap();
    let gateway = scratch.path().join("gateway");
    let workspace = gateway.join("workspaces/demo");
    let private = scratch.path().join("private");
    for dir in [&workspace, &gateway.join("config"), &private] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(gateway.join("config/key"), "hidden").unwrap();
    std::fs::write(private.join("key"), "hidden").unwrap();
    let policy = Policy {
        hidden: vec![gateway.clone(), private.clone()],
        open_within_hidden: vec![gateway.join("workspaces")],
        ..Default::default()
    };
    let sandbox = Sandbox::enforced_for_test(policy);
    let paths = command_guard::SecretPaths::for_sandbox(&sandbox, &workspace);
    for granted in [false, true] {
        for (name, target) in [
            ("gateway-link", gateway.join("config")),
            ("private-link", private.clone()),
        ] {
            let link = workspace.join(format!("{name}-{granted}"));
            std::os::unix::fs::symlink(target, &link).unwrap();
            let file = link.join("key");
            let command = format!("cat '{}'", file.display());
            assert_eq!(
                command_guard::grant_covers_command_with_paths(
                    "Bash",
                    &command,
                    CommandSite::TextOnly { root: None },
                    &TrustedHeads::NONE,
                    &paths,
                    |_| granted
                ),
                granted
            );
            let output = sandbox
                .command("/bin/cat")
                .unwrap()
                .arg(&file)
                .output()
                .await
                .unwrap();
            assert!(
                !output.status.success(),
                "a link escaped the executing sandbox"
            );
            assert!(output.stdout.is_empty());
        }
    }
}

mod guard_tests {
    /// The owner's condition: the confinement run is the one thing that runs
    /// outside the sandbox. One constructor builds that mode, and one production
    /// caller uses it.
    #[test]
    fn the_confinement_route_is_the_only_bare_spawn() {
        let sources = crate::test_support::source_scan::production_sources();
        let sites = |needle: &str| -> Vec<String> {
            sources
                .iter()
                .filter(|(_, text)| text.contains(needle))
                .map(|(path, text)| format!("{path} ×{}", text.matches(needle).count()))
                .collect()
        };
        assert_eq!(
            sites("(Mode::PermittedOutside)"),
            ["core/agent_sandbox.rs ×1"],
            "only outside_for_confinement may build the bare mode"
        );
        let mut callers = sites("outside_for_confinement(");
        callers.sort();
        assert_eq!(
            callers,
            [
                "core/agent_sandbox.rs ×1",
                "engine/agent_session/confinement_run.rs ×1",
            ],
            "only the confinement route may run bare, beside the definition"
        );
    }

    /// Every covered spawn site (decision 6 of
    /// `docs/plans/2026-11-10-os-sandbox-for-agent-subprocesses.md`) builds its
    /// child from a [`Sandbox`]. A raw command in one of them is the hole this
    /// pins shut. Each allowed raw command names why it runs no agent code.
    #[test]
    fn every_covered_spawn_site_builds_from_a_sandbox() {
        use crate::test_support::source_scan::{engine_source, read_production_source};
        let covered: [(&str, &[&str]); 12] = [
            ("engine/tools/bash.rs", &[]),
            // `ps` reads a sidecar's pid before any agent code runs.
            (
                "engine/frontend_preview.rs",
                &["std::process::Command::new(\"ps\")"],
            ),
            ("engine/bash_background.rs", &[]),
            // The venv is made by the system python, before any agent code.
            (
                "runtime/python.rs",
                &["tokio::process::Command::new(\"python3\")"],
            ),
            ("engine/engine_impl/scripts.rs", &[]),
            ("api/proxy_script_runner.rs", &[]),
            ("engine/agent_session/background_task.rs", &[]),
            ("mcp/client.rs", &[]),
            ("runtime/claude_code.rs", &[]),
            // Never spawned: it only collects the env the turns re-apply.
            (
                "runtime/codex.rs",
                &["tokio::process::Command::new(\"true\")"],
            ),
            ("runtime/codex_app_server.rs", &[]),
            ("core/shell.rs", &[]),
        ];
        for (file, allowed) in covered {
            let source = read_production_source(&engine_source(file));
            let mut rest = source.clone();
            for raw in allowed {
                assert!(rest.contains(raw), "{file}: the allowed `{raw}` is gone");
                rest = rest.replacen(raw, "", 1);
            }
            let code: String = rest
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("Command::new("),
                "{file} builds a raw command; build it from a Sandbox"
            );
            assert!(
                code.contains("Sandbox") || code.contains("sandbox"),
                "{file} no longer reaches the sandbox"
            );
        }
    }
}

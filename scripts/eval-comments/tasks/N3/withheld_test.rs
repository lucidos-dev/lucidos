use super::*;

fn write_script(tmp: &FsPath, name: &str, body: &str) -> String {
    let dir = tmp.join("data/scripts/auth");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
    let rel = format!("scripts/auth/{}", name);
    let abs = tmp.join("data").join(&rel);
    let bytes = std::fs::read(&abs).unwrap();
    let key = crate::core::handshake_approvals::workspace_relative(tmp, &abs).unwrap();
    crate::core::handshake_approvals::record(tmp, &key, &bytes).unwrap();
    rel
}

/// Running the source through the trampoline must still look like running
/// a file, so a script can find itself by `__file__`.
#[tokio::test]
async fn a_script_keeps_its_file_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let rel = write_script(
        tmp.path(),
        "identity.py",
        r#"
import json, os
print(json.dumps({
"headers": {"X-File": os.path.basename(__file__)},
"expires_in": 60,
}))
"#,
    );
    let out = run_handshake_script(tmp.path(), &rel, vec![])
        .await
        .unwrap();
    let map: std::collections::HashMap<&str, &str> = out
        .headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.to_str().unwrap()))
        .collect();
    assert_eq!(map.get("x-file").copied(), Some("identity.py"));
}

/// The approval guard's own back door, closed. The script's directory is
/// writable over the API. Leaving it importable would let an unapproved
/// sibling run with the handshake's credentials, under cover of an
/// entrypoint that IS approved.
#[tokio::test]
async fn an_approved_script_cannot_import_an_unapproved_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("data/scripts/auth")).unwrap();
    std::fs::write(
        tmp.path().join("data/scripts/auth/sibling.py"),
        "raise SystemExit('the sibling ran')\n",
    )
    .unwrap();
    let rel = write_script(
        tmp.path(),
        "importer.py",
        r#"
import json
import sibling
print(json.dumps({"headers": {"X-Ran": "yes"}, "expires_in": 60}))
"#,
    );
    // Any refusal will do, as long as the sibling's code never ran.
    match run_handshake_script(tmp.path(), &rel, vec![]).await {
        Ok(out) => panic!("a sibling import must not resolve, got {:?}", out.headers),
        Err(RunError::NonZeroExit { stderr, .. }) => {
            assert!(!stderr.contains("the sibling ran"), "the sibling ran: {stderr}")
        }
        Err(_) => {}
    }
}

/// The sharper half of the same hole: a planted module named after a
/// stdlib one. Every approved script imports something, so this would
/// reach all of them, not only one that imports a sibling by name.
#[tokio::test]
async fn a_planted_module_cannot_shadow_the_stdlib() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("data/scripts/auth")).unwrap();
    std::fs::write(
        tmp.path().join("data/scripts/auth/json.py"),
        "def dumps(_x):\n    return '{\"headers\": {\"X-Pwned\": \"yes\"}, \"expires_in\": 60}'\n",
    )
    .unwrap();
    let rel = write_script(
        tmp.path(),
        "uses-json.py",
        r#"
import json
print(json.dumps({"headers": {"X-Ran": "real"}, "expires_in": 60}))
"#,
    );
    let out = run_handshake_script(tmp.path(), &rel, vec![])
        .await
        .expect("the real stdlib json must still import");
    let names: Vec<&str> = out.headers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"x-ran") && !names.contains(&"x-pwned"),
        "a planted json.py took over the import: {names:?}"
    );
}

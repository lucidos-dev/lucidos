use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::arm::Arm;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const EVAL_WORKSPACE_PREFIX: &str = "eval-";

pub const SEEDED_TABLES: [&str; 3] = ["preferences", "memory_entries", "models"];

pub fn checked_eval_workspace(path: &Path) -> Fallible<PathBuf> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| refusal(path, "it has no final path component"))?;
    if name == ".." || name == "." {
        return Err(refusal(path, "it ends in a relative component"));
    }
    if !name.starts_with(EVAL_WORKSPACE_PREFIX) {
        return Err(refusal(
            path,
            &format!("its name does not start with `{EVAL_WORKSPACE_PREFIX}`"),
        ));
    }
    if !path.is_absolute() {
        return Err(refusal(path, "it is not an absolute path"));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(refusal(path, "it is a symlink, and writes would follow"));
        }
        if !metadata.is_dir() {
            return Err(refusal(path, "it exists and is not a directory"));
        }
    }
    Ok(path.to_path_buf())
}

pub fn create_seed_workspace(path: &Path) -> Fallible<()> {
    checked_eval_workspace(path)?;
    let parent = path.parent().ok_or("workspace has no parent")?;
    std::fs::create_dir_all(parent)?;
    std::fs::create_dir(path)
        .map_err(|error| refusal(path, &format!("exclusive creation failed: {error}")))
}

fn refusal(path: &Path, why: &str) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "refusing to use {} as an eval workspace: {why}. Choose a fresh run label \
         for a new seed, or resume the recorded run without reseeding.",
        path.display()
    )
    .into()
}

const MAX_LABEL_BYTES: usize = 31;

pub const MAX_REPEAT: u32 = 99_999;

const LABEL_DIGEST_HEX: usize = 6;

const MAX_LABEL_STEM_BYTES: usize = MAX_LABEL_BYTES - LABEL_DIGEST_HEX - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLabel(String);

impl RunLabel {
    pub fn derive(source: &str) -> RunLabel {
        let stem = sanitise(source);
        let digest = hex(Sha256::digest(source.as_bytes()).as_slice());
        let digest = &digest[..LABEL_DIGEST_HEX];
        let stem = match stem.len() > MAX_LABEL_STEM_BYTES {
            true => stem[..MAX_LABEL_STEM_BYTES].trim_end_matches('-'),
            false => &stem,
        };
        match stem.is_empty() {
            true => RunLabel(digest.to_string()),
            false => RunLabel(format!("{stem}-{digest}")),
        }
    }

    pub fn recorded(label: &str) -> RunLabel {
        RunLabel(sanitise(label))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for RunLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn sanitise(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for ch in source.chars().flat_map(char::to_lowercase) {
        match ch.is_ascii_alphanumeric() {
            true => out.push(ch),
            false => {
                if !out.ends_with('-') {
                    out.push('-');
                }
            }
        }
    }
    out.trim_matches('-').to_string()
}

pub fn arm_workspace_name(label: &RunLabel, arm: Arm, repeat: u32) -> String {
    match label.is_empty() {
        true => format!("{EVAL_WORKSPACE_PREFIX}{arm}-{repeat}"),
        false => format!("{EVAL_WORKSPACE_PREFIX}{label}-{arm}-{repeat}"),
    }
}

pub fn arm_workspace_path(eval_root: &Path, label: &RunLabel, arm: Arm, repeat: u32) -> PathBuf {
    eval_root.join(arm_workspace_name(label, arm, repeat))
}

pub fn arm_database_name(label: &RunLabel, arm: Arm, repeat: u32) -> String {
    format!("lucidos_{}", arm_workspace_name(label, arm, repeat))
}

pub fn arm_database_url(pg_base: &str, label: &RunLabel, arm: Arm, repeat: u32) -> String {
    format!(
        "{}/{}",
        pg_base.trim_end_matches('/'),
        arm_database_name(label, arm, repeat)
    )
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeedRow {
    pub table: String,
    pub key: String,
    pub value: String,
}

impl SeedRow {
    pub fn new(table: &str, key: impl Into<String>, value: impl Into<String>) -> Self {
        SeedRow {
            table: table.to_string(),
            key: key.into(),
            value: value.into(),
        }
    }
}

pub fn digest_rows(rows: &[SeedRow]) -> String {
    let mut sorted: Vec<&SeedRow> = rows.iter().collect();
    sorted.sort();
    let mut hasher = Sha256::new();
    for row in sorted {
        hasher.update(row.table.as_bytes());
        hasher.update([0x1f]);
        hasher.update(row.key.as_bytes());
        hasher.update([0x1f]);
        hasher.update(row.value.as_bytes());
        hasher.update([0x1e]);
    }
    hex(hasher.finalize().as_slice())
}

pub async fn read_seed_rows(pool: &PgPool) -> Fallible<Vec<SeedRow>> {
    let mut rows = Vec::new();

    let prefs = sqlx::query(
        "SELECT key, COALESCE(device_id, '') AS device_id, value \
         FROM preferences WHERE key <> ALL($1)",
    )
    .bind(crate::arm::ARM_PREFERENCE_KEYS.map(str::to_string).to_vec())
    .fetch_all(pool)
    .await?;
    for row in prefs {
        let key: String = row.try_get("key")?;
        let device_id: String = row.try_get("device_id")?;
        let value: String = row.try_get("value")?;
        rows.push(SeedRow::new(
            "preferences",
            format!("{key}\u{1d}{device_id}"),
            value,
        ));
    }

    let memories = sqlx::query(
        "SELECT id::text AS id, source::text AS source, topic, summary, importance::text \
         AS importance, entities::text AS entities, embedding::text AS embedding, \
         embedding_model, src_created_at::text AS src_created_at, \
         extractor_version::text AS extractor_version FROM memory_entries",
    )
    .fetch_all(pool)
    .await?;
    for row in memories {
        let id: String = row.try_get("id")?;
        let mut value = String::new();
        for column in [
            "source",
            "topic",
            "summary",
            "importance",
            "entities",
            "embedding",
            "embedding_model",
            "src_created_at",
            "extractor_version",
        ] {
            let cell: String = row.try_get(column)?;
            value.push_str(&cell);
            value.push('\u{1d}');
        }
        rows.push(SeedRow::new("memory_entries", id, value));
    }

    let models = sqlx::query(
        "SELECT id, label, routes::text AS routes, sort_order::text AS sort_order, source \
         FROM models",
    )
    .fetch_all(pool)
    .await?;
    for row in models {
        let id: String = row.try_get("id")?;
        let mut value = String::new();
        for column in ["label", "routes", "sort_order", "source"] {
            let cell: String = row.try_get(column)?;
            value.push_str(&cell);
            value.push('\u{1d}');
        }
        rows.push(SeedRow::new("models", id, value));
    }

    Ok(rows)
}

pub fn fs_digest(workspace: &Path) -> Fallible<String> {
    let root = workspace.join("data");
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(&root, &root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for (relative, absolute) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0x1f]);
        hasher.update(Sha256::digest(std::fs::read(&absolute)?).as_slice());
        hasher.update([0x1e]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Fallible<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })?
                .to_string_lossy()
                .into_owned();
            out.push((relative, path));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedDigest {
    pub db: String,
    pub fs: String,
}

pub fn compare_digests(control: &SeedDigest, lean: &SeedDigest) -> Fallible<()> {
    let mut differences = Vec::new();
    if control.db != lean.db {
        differences.push(format!(
            "the seeded tables ({}) differ: {} against {}. The digest already excludes every \
             permitted difference: {}",
            SEEDED_TABLES.join(", "),
            control.db,
            lean.db,
            crate::arm::ARM_PREFERENCE_KEYS.join(", ")
        ));
    }
    if control.fs != lean.fs {
        differences.push(format!(
            "the data trees differ: {} against {}",
            control.fs, lean.fs
        ));
    }
    if differences.is_empty() {
        return Ok(());
    }
    Err(format!(
        "seed_digest_mismatch: the two arms are not identically seeded, so any result would \
         carry an uncontrolled variable. {}",
        differences.join("; ")
    )
    .into())
}

pub fn install_fixture_tree(workspace: &Path, fixture_root: &Path) -> Fallible<()> {
    checked_eval_workspace(workspace)?;
    let data = workspace.join("data");
    std::fs::create_dir(&data)?;
    copy_tree(fixture_root, &data)
}

fn copy_tree(from: &Path, to: &Path) -> Fallible<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

pub struct BootedEngine {
    pub child: std::process::Child,
    pub base_url: String,
}

pub fn create_database(pg_base: &str, database: &str) -> Fallible<()> {
    let admin = format!("{}/postgres", pg_base.trim_end_matches('/'));
    psql(&admin, &["-c", &create_database_statement(database)])
}

fn create_database_statement(database: &str) -> String {
    format!("CREATE DATABASE {}", quote_ident(database))
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn migrate_by_booting(engine_bin: &Path, workspace: &Path, database_url: &str) -> Fallible<()> {
    let engine = boot_engine(engine_bin, workspace, database_url, free_port()?, false)?;
    stop_engine(engine)
}

pub fn boot_engine(
    engine_bin: &Path,
    workspace: &Path,
    database_url: &str,
    port: u16,
    full_capture: bool,
) -> Fallible<BootedEngine> {
    checked_eval_workspace(workspace)?;
    let child = std::process::Command::new(engine_bin)
        .envs(arm_engine_env(workspace, database_url, port, full_capture))
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", engine_bin.display()))?;
    let base_url = format!("http://127.0.0.1:{port}");
    let mut child = child;
    if let Err(error) = wait_for_health(&mut child, &base_url, Duration::from_secs(120)) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(BootedEngine { child, base_url })
}

fn arm_engine_env(
    workspace: &Path,
    database_url: &str,
    port: u16,
    full_capture: bool,
) -> Vec<(&'static str, OsString)> {
    let mut env: Vec<(&'static str, OsString)> = vec![
        ("LUCIDOS_WORKSPACE", workspace.as_os_str().to_os_string()),
        ("DATABASE_URL", OsString::from(database_url)),
        ("LUCIDOS_API_PORT", OsString::from(port.to_string())),
        (
            FORCE_QUERY_CLASSIFICATION_ENV,
            OsString::from(FORCE_QUERY_CLASSIFICATION_VALUE),
        ),
        (BIND_LOOPBACK_ENV, OsString::from("1")),
        ("LUCIDOS_TLS_CERT", OsString::new()),
        ("LUCIDOS_TLS_KEY", OsString::new()),
    ];
    if full_capture {
        env.push((FULL_CAPTURE_ENV, OsString::from("1")));
    }
    env
}

pub const FULL_CAPTURE_ENV: &str = "LUCIDOS_EVAL_FULL_CAPTURE";

const BIND_LOOPBACK_ENV: &str = "LUCIDOS_BIND_LOOPBACK";

const FORCE_QUERY_CLASSIFICATION_ENV: &str = "LUCIDOS_FORCE_QUERY_CLASSIFICATION";

const FORCE_QUERY_CLASSIFICATION_VALUE: &str = "all";

pub fn stop_engine(mut engine: BootedEngine) -> Fallible<()> {
    engine.child.kill()?;
    engine.child.wait()?;
    Ok(())
}

fn wait_for_health(
    child: &mut std::process::Child,
    base_url: &str,
    timeout: Duration,
) -> Fallible<()> {
    let deadline = std::time::Instant::now() + timeout;
    let url = format!("{base_url}/api/v1/health");
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "engine_exited_during_boot: the engine left with {status} before answering \
                 {url}. Another process may already hold that port: stop the workspace in \
                 the picker, or check the engine's own output."
            )
            .into());
        }
        let healthy = std::process::Command::new("curl")
            .args(["-sf", "-o", "/dev/null", "--max-time", "2", &url])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if healthy {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "engine_never_healthy: {url} did not answer within {}s",
                timeout.as_secs()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn free_port() -> Fallible<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

pub fn wait_for_port_free(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub struct SeedPins<'a> {
    pub model: &'a str,
    pub model_label: &'a str,
    pub model_provider: &'a str,
    pub reasoning_effort: &'a str,
    pub background_model: &'a str,
    pub context_window: Option<i64>,
}

pub fn apply_seed_sql(database_url: &str, seed_sql: &Path, pins: &SeedPins) -> Fallible<()> {
    let window = match pins.context_window {
        Some(window) => window.to_string(),
        None => "NULL".to_string(),
    };
    psql(
        database_url,
        &[
            "-v",
            "ON_ERROR_STOP=1",
            "-v",
            &format!("model={}", pins.model),
            "-v",
            &format!("model_label={}", pins.model_label),
            "-v",
            &format!("model_provider={}", pins.model_provider),
            "-v",
            &format!("reasoning_effort={}", pins.reasoning_effort),
            "-v",
            &format!("background_model={}", pins.background_model),
            "-v",
            &format!("context_window={window}"),
            "-f",
            &seed_sql.to_string_lossy(),
        ],
    )
}

pub async fn write_arm_preference(
    pool: &PgPool,
    arm: Arm,
    sweep: crate::arm::SweepPins,
) -> Fallible<()> {
    for (key, value) in arm.preference_rows(sweep) {
        sqlx::query(
            "INSERT INTO preferences (key, value, device_id, updated_at) \
             VALUES ($1, $2, NULL, '2026-01-01 00:00:00+00')",
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    }
    Ok(())
}

fn psql(database_url: &str, args: &[&str]) -> Fallible<()> {
    let url = lucidos_pg_url::PgUrl::parse(database_url)?;
    let output = std::process::Command::new("psql")
        .envs(url.env())
        .args(args)
        .output()
        .map_err(|e| format!("cannot run psql: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "psql failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
    .into())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<SeedRow> {
        vec![
            SeedRow::new("preferences", "timezone\u{1d}", "Europe/Oslo"),
            SeedRow::new("preferences", "language\u{1d}", "English"),
            SeedRow::new(
                "models",
                "claude-haiku-4-5",
                "Haiku 4.5\u{1d}anthropic\u{1d}",
            ),
            SeedRow::new("memory_entries", "0b1e", "topic\u{1d}summary\u{1d}"),
        ]
    }

    #[test]
    fn seed_refuses_an_existing_directory_without_changing_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("eval-existing-lean-1");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("user-data"), "keep").unwrap();
        assert!(create_seed_workspace(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("user-data")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn seed_exclusively_creates_a_new_workspace() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("eval-new-lean-1");
        create_seed_workspace(&path).unwrap();
        assert!(path.is_dir());
        assert!(create_seed_workspace(&path).is_err());
    }

    #[test]
    fn a_workspace_without_the_eval_prefix_is_refused() {
        let err = checked_eval_workspace(Path::new("/tmp/workspaces/dev")).unwrap_err();
        assert!(err.to_string().contains("does not start with `eval-`"));
    }

    #[test]
    fn a_relative_eval_path_is_refused() {
        let err = checked_eval_workspace(Path::new("eval-lean-1")).unwrap_err();
        assert!(err.to_string().contains("not an absolute path"));
    }

    #[test]
    fn an_eval_prefixed_absolute_path_is_accepted() {
        let path = Path::new("/tmp/eval-root/eval-control-1");
        assert_eq!(checked_eval_workspace(path).unwrap(), path);
    }

    #[test]
    fn a_correctly_named_symlink_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("somebody-elses-workspace");
        let link = root.path().join("eval-lean-1");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = checked_eval_workspace(&link).unwrap_err().to_string();
        assert!(err.contains("it is a symlink"), "got: {err}");
    }

    #[test]
    fn a_real_directory_with_the_prefix_is_still_accepted() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("eval-lean-1");
        std::fs::create_dir_all(&workspace).unwrap();
        checked_eval_workspace(&workspace).unwrap();
    }

    fn opus() -> RunLabel {
        RunLabel::derive("claude-opus-5@default")
    }

    #[test]
    fn the_label_arm_and_repeat_name_the_workspace_and_the_database() {
        let root = Path::new("/tmp/eval-root");
        let label = opus();
        assert_eq!(
            arm_workspace_path(root, &label, Arm::LEAN, 3),
            root.join(format!("eval-{label}-lean-3"))
        );
        assert_eq!(
            arm_database_name(&label, Arm::CONTROL, 2),
            format!("lucidos_eval-{label}-control-2")
        );
        assert_eq!(
            arm_database_url("postgres://u:p@localhost:5438/", &label, Arm::LEAN, 1),
            format!("postgres://u:p@localhost:5438/lucidos_eval-{label}-lean-1")
        );
    }

    #[test]
    fn two_models_never_share_a_workspace_or_a_database() {
        let anthropic = RunLabel::derive("claude-opus-5@default");
        let openai = RunLabel::derive("gpt-5.6-sol");
        assert_ne!(
            arm_workspace_name(&anthropic, Arm::LEAN, 1),
            arm_workspace_name(&openai, Arm::LEAN, 1)
        );
        assert_ne!(
            arm_database_name(&anthropic, Arm::LEAN, 1),
            arm_database_name(&openai, Arm::LEAN, 1)
        );
    }

    #[test]
    fn a_model_id_keeps_a_recognisable_stem() {
        for (id, stem) in [
            ("claude-opus-5@default", "claude-opus-5-default"),
            ("claude-opus-5@default[1m]", "claude-opus-5-default-1m"),
            ("gpt-5.6-sol", "gpt-5-6-sol"),
            ("stealth/ox-alpha", "stealth-ox-alpha"),
            ("Claude-Sonnet-5", "claude-sonnet-5"),
            ("--messy--id--", "messy-id"),
        ] {
            let label = RunLabel::derive(id);
            assert!(
                label.as_str().starts_with(&format!("{stem}-")),
                "{id} gave {label}, which does not read as {stem}"
            );
            assert_eq!(
                label.as_str().len(),
                stem.len() + 1 + LABEL_DIGEST_HEX,
                "{id} gave {label}, which is stem plus digest and nothing else"
            );
        }
    }

    #[test]
    fn two_ids_that_sanitise_alike_still_get_different_labels() {
        for (left, right) in [
            ("gpt-5.6", "gpt-5-6"),
            ("Claude-Opus-5", "claude-opus-5"),
            ("stealth/ox-alpha", "stealth.ox.alpha"),
        ] {
            let first = RunLabel::derive(left);
            let second = RunLabel::derive(right);
            assert_eq!(sanitise(left), sanitise(right), "the premise of the pair");
            assert_ne!(first, second, "{left} collided with {right}");
            assert_ne!(
                arm_database_name(&first, Arm::LEAN, 1),
                arm_database_name(&second, Arm::LEAN, 1)
            );
        }
    }

    #[test]
    fn a_labelled_workspace_still_carries_the_eval_prefix() {
        let name = arm_workspace_name(&opus(), Arm::CONTROL, 7);
        assert!(name.starts_with(EVAL_WORKSPACE_PREFIX), "{name}");
        assert!(name.ends_with("-control-7"), "{name}");
    }

    #[test]
    fn the_longest_possible_name_fits_a_postgres_identifier() {
        let longest_arm = Arm::ALL
            .into_iter()
            .max_by_key(|arm| arm.as_str().len())
            .expect("there are arms");
        let longest_label = RunLabel("z".repeat(MAX_LABEL_BYTES));
        let name = arm_database_name(&longest_label, longest_arm, MAX_REPEAT);
        assert!(name.len() <= 63, "{} bytes: {name}", name.len());

        let over = RunLabel("z".repeat(MAX_LABEL_BYTES + 1));
        assert_eq!(
            arm_database_name(&over, longest_arm, MAX_REPEAT).len(),
            64,
            "the ceiling is exactly one byte above the longest legal label"
        );
    }

    #[test]
    fn two_long_model_ids_sharing_a_prefix_get_different_labels() {
        let first = RunLabel::derive("some-very-long-provider/experimental-model-alpha-2026");
        let second = RunLabel::derive("some-very-long-provider/experimental-model-beta-2026");
        assert_ne!(first, second, "{first} collided with {second}");
        assert!(first.as_str().len() <= MAX_LABEL_BYTES, "{first}");
        assert!(second.as_str().len() <= MAX_LABEL_BYTES, "{second}");
        assert_ne!(
            arm_database_name(&first, Arm::LEAN, 1),
            arm_database_name(&second, Arm::LEAN, 1)
        );
    }

    #[test]
    fn a_derived_label_is_stable_across_calls() {
        let id = "some-very-long-provider/experimental-model-alpha-2026";
        assert_eq!(RunLabel::derive(id), RunLabel::derive(id));
    }

    #[test]
    fn a_recorded_label_reads_back_as_the_label_that_was_written() {
        for id in [
            "claude-opus-5@default",
            "gpt-5.6-sol",
            "some-very-long-provider/experimental-model-alpha-2026",
        ] {
            let written = RunLabel::derive(id);
            assert_eq!(RunLabel::recorded(written.as_str()), written, "from {id}");
        }
    }

    #[test]
    fn a_recorded_label_beats_whatever_the_shell_is_pinned_to() {
        let recorded = RunLabel::recorded("gpt-5-6-sol");
        let ambient = RunLabel::derive("claude-opus-5@default");
        assert_ne!(
            arm_database_name(&recorded, Arm::LEAN, 1),
            arm_database_name(&ambient, Arm::LEAN, 1)
        );
        assert_eq!(
            arm_database_name(&recorded, Arm::LEAN, 1),
            "lucidos_eval-gpt-5-6-sol-lean-1"
        );
    }

    #[test]
    fn an_unlabelled_run_keeps_the_name_it_was_created_with() {
        let none = RunLabel::recorded("");
        assert_eq!(arm_workspace_name(&none, Arm::LEAN, 1), "eval-lean-1");
        assert_eq!(
            arm_database_name(&none, Arm::LEAN, 1),
            "lucidos_eval-lean-1"
        );
        assert!(arm_workspace_name(&none, Arm::CONTROL, 2).starts_with(EVAL_WORKSPACE_PREFIX));
    }

    #[test]
    fn an_id_that_sanitises_to_nothing_still_yields_a_label() {
        let label = RunLabel::derive("///");
        assert!(!label.as_str().is_empty());
        assert!(arm_database_name(&label, Arm::LEAN, 1).len() <= 63);
    }

    #[test]
    fn the_database_name_is_what_the_gateway_derives_from_the_slug() {
        let label = RunLabel::recorded("m");
        assert_eq!(arm_workspace_name(&label, Arm::LEAN, 1), "eval-m-lean-1");
        assert_eq!(
            arm_database_name(&label, Arm::LEAN, 1),
            "lucidos_eval-m-lean-1"
        );
        assert!(!arm_workspace_name(&label, Arm::LEAN, 1).contains('_'));
    }

    #[test]
    fn the_hyphenated_database_name_is_quoted_in_every_statement() {
        let name = arm_database_name(&RunLabel::recorded("m"), Arm::LEAN, 1);
        assert_eq!(
            create_database_statement(&name),
            "CREATE DATABASE \"lucidos_eval-m-lean-1\""
        );
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    fn arm_env() -> Vec<(&'static str, OsString)> {
        arm_engine_env(
            Path::new("/tmp/eval-lean-1"),
            "postgres://localhost/lucidos_eval-lean-1",
            3210,
            true,
        )
    }

    fn value_of<'a>(env: &'a [(&'static str, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    #[test]
    fn every_arm_boots_with_the_classifier_pinned_to_all() {
        let env = arm_env();
        assert_eq!(
            value_of(&env, "LUCIDOS_FORCE_QUERY_CLASSIFICATION"),
            Some(&OsString::from("all")),
            "the pin is missing from an arm's environment"
        );
    }

    #[test]
    fn every_arm_boots_pinned_to_loopback() {
        let env = arm_env();
        assert_eq!(
            value_of(&env, BIND_LOOPBACK_ENV),
            Some(&OsString::from("1"))
        );
    }

    #[test]
    fn the_pin_is_all_and_not_none() {
        assert_eq!(FORCE_QUERY_CLASSIFICATION_VALUE, "all");
    }

    #[test]
    fn the_pin_did_not_displace_what_the_engine_already_needed() {
        let env = arm_env();
        assert_eq!(
            value_of(&env, "LUCIDOS_WORKSPACE"),
            Some(&OsString::from("/tmp/eval-lean-1"))
        );
        assert_eq!(
            value_of(&env, "DATABASE_URL"),
            Some(&OsString::from("postgres://localhost/lucidos_eval-lean-1"))
        );
        assert_eq!(
            value_of(&env, "LUCIDOS_API_PORT"),
            Some(&OsString::from("3210"))
        );
    }

    #[test]
    fn every_arm_serves_plain_http() {
        let env = arm_env();
        assert_eq!(value_of(&env, "LUCIDOS_TLS_CERT"), Some(&OsString::new()));
        assert_eq!(value_of(&env, "LUCIDOS_TLS_KEY"), Some(&OsString::new()));
    }

    #[test]
    fn the_arm_environment_names_no_provider_credential() {
        let env = arm_env();
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "OPENROUTER_API_KEY",
            "XAI_API_KEY",
        ] {
            assert_eq!(
                value_of(&env, key),
                None,
                "{key} is pinned per arm, so a concurrent run against another provider \
                 cannot reach its own credential"
            );
        }
        assert!(!SEEDED_TABLES.contains(&"credentials"));
    }

    #[test]
    fn the_digest_ignores_the_order_rows_arrived_in() {
        let forward = rows();
        let mut backward = rows();
        backward.reverse();
        assert_eq!(digest_rows(&forward), digest_rows(&backward));
    }

    #[test]
    fn mutating_one_seeded_row_fails_the_digest_comparison() {
        let control = SeedDigest {
            db: digest_rows(&rows()),
            fs: "tree".into(),
        };
        let mut mutated = rows();
        mutated[0].value = "Europe/Berlin".into();
        let lean = SeedDigest {
            db: digest_rows(&mutated),
            fs: "tree".into(),
        };
        assert_ne!(control.db, lean.db);
        let err = compare_digests(&control, &lean).unwrap_err().to_string();
        assert!(err.contains("seed_digest_mismatch"));
        assert!(err.contains("the seeded tables"));
    }

    #[test]
    fn identical_seeding_passes_and_the_arm_row_is_excluded() {
        let digest = SeedDigest {
            db: digest_rows(&rows()),
            fs: "tree".into(),
        };
        compare_digests(&digest, &digest).unwrap();
        for key in crate::arm::ARM_PREFERENCE_KEYS {
            assert!(!rows().iter().any(|r| r.key.starts_with(key)));
        }
    }

    #[test]
    fn a_differing_data_tree_fails_the_comparison() {
        let control = SeedDigest {
            db: "same".into(),
            fs: "tree-a".into(),
        };
        let lean = SeedDigest {
            db: "same".into(),
            fs: "tree-b".into(),
        };
        let err = compare_digests(&control, &lean).unwrap_err().to_string();
        assert!(err.contains("the data trees differ"));
    }
}

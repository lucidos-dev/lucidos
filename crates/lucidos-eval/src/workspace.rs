//! Create, seed, digest, boot, stop and tear down one arm's workspace.
//!
//! Two guarantees live here and both fail closed. I5: nothing outside a
//! directory named `eval-…` is ever written to or dropped. I1: the two arms are
//! proved byte-identical, in the three seeded tables and in the whole data
//! tree, before the first prompt is sent.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::arm::Arm;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Every eval workspace directory name starts with this. The harness refuses
/// anything else, because seeding drops a database and clears a data tree.
pub const EVAL_WORKSPACE_PREFIX: &str = "eval-";

/// Tables the seed writes and the digest covers. Nothing else is seeded, so
/// anything else differing between the arms came from the run itself.
pub const SEEDED_TABLES: [&str; 3] = ["preferences", "memory_entries", "models"];

/// Reject any workspace path the harness is not allowed to touch (I5).
///
/// Two things are checked. The final path component is the workspace name the
/// operator chose, and it must carry the prefix. The path itself must not be a
/// symlink: a name check alone passes `eval-lean-1` pointing at a live
/// workspace, and clearing the data tree would follow it.
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
    // `symlink_metadata` does not follow the link, which is the whole point.
    // An absent path is fine: the first seed creates it.
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(refusal(
                path,
                "it is a symlink, and clearing it would follow",
            ));
        }
        if !metadata.is_dir() {
            return Err(refusal(path, "it exists and is not a directory"));
        }
    }
    Ok(path.to_path_buf())
}

fn refusal(path: &Path, why: &str) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "refusing to use {} as an eval workspace: {why}. The harness drops this \
         workspace's database and clears its data tree, so it only ever runs \
         against a directory it created.",
        path.display()
    )
    .into()
}

/// Longest a run label may be, in bytes.
///
/// A Postgres identifier holds 63 bytes. The longest name built here spends 32
/// of them on `lucidos_`, `eval-`, the separators, `control` and a ten-digit
/// repeat. That leaves 31. The guard is
/// `the_longest_possible_name_fits_a_postgres_identifier`, which computes the
/// fixed part rather than trusting this line.
const MAX_LABEL_BYTES: usize = 31;

/// Hex digits of the digest every derived label ends in.
const LABEL_DIGEST_HEX: usize = 6;

/// Longest the readable part of a label may be, before its digest.
const MAX_LABEL_STEM_BYTES: usize = MAX_LABEL_BYTES - LABEL_DIGEST_HEX - 1;

/// What separates one run's arm workspaces from another's.
///
/// An **arm** is a context-mode configuration and stays one. The model is a
/// separate axis. Without it in the name, two runs against different providers
/// both want `eval-lean-1` and both try to create `lucidos_eval-lean-1`. The
/// second corrupts or fails against the first.
///
/// Sanitised on construction, so every name built from it is already legal as
/// both a directory basename and a Postgres identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLabel(String);

impl RunLabel {
    /// Derive a label from a model id, or from whatever an operator set.
    ///
    /// Two parts, always both: a readable stem and a digest of the WHOLE
    /// source. `claude-opus-5@default` becomes `claude-opus-5-default-` plus
    /// six hex digits, which is still what an operator recognises when opening
    /// the database by hand.
    ///
    /// **The digest is not only for length.** Sanitising is lossy, so it merges
    /// ids as readily as truncating does: `gpt-5.6` and `gpt-5-6` reduce to one
    /// stem, and so does any pair differing only in case or punctuation. Two
    /// such runs would share a database, which is the isolation this type
    /// exists to provide. Deriving the digest from the untouched source is what
    /// makes the label injective, whatever the stem did.
    pub fn derive(source: &str) -> RunLabel {
        let stem = sanitise(source);
        let digest = hex(Sha256::digest(source.as_bytes()).as_slice());
        let digest = &digest[..LABEL_DIGEST_HEX];
        // Sanitising leaves pure ASCII, so a byte index is a char boundary.
        let stem = match stem.len() > MAX_LABEL_STEM_BYTES {
            true => stem[..MAX_LABEL_STEM_BYTES].trim_end_matches('-'),
            false => &stem,
        };
        match stem.is_empty() {
            true => RunLabel(digest.to_string()),
            false => RunLabel(format!("{stem}-{digest}")),
        }
    }

    /// The label a results file recorded, for a command that reads one back.
    ///
    /// A post-run command must resolve the arm's database from the file rather
    /// than from its own environment. Otherwise `score` in a shell pinned to
    /// one model reads the other model's arms.
    ///
    /// This is a name read back, never a source to derive from: re-deriving
    /// would digest the digest and name a database nothing created. Sanitising
    /// is kept, because it is a no-op on anything [`RunLabel::derive`] wrote
    /// and it contains a hand-edited results file.
    ///
    /// Empty for a run recorded before the label existed. Those workspaces
    /// carry no label segment at all, so the name comes back out as it went in.
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

/// Reduce a string to `[a-z0-9-]`, collapsed and trimmed.
///
/// Lossy on purpose, so the result reads like the id it came from. What makes a
/// label unique is the digest beside it, never this.
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

/// Directory name for this run's arm and repeat, also its workspace address.
///
/// The gateway derives a workspace's slug from its directory basename and its
/// database from that slug, so this one string decides all three. Every other
/// name here is built from it rather than spelled again.
///
/// The label leads so one run's workspaces sort together, in the picker and in
/// `ls`. Arm and repeat stay where the eye already reads them.
///
/// An empty label drops its segment, which is the name a run recorded before
/// the label existed used. Scoring or replaying one still has to find it.
pub fn arm_workspace_name(label: &RunLabel, arm: Arm, repeat: u32) -> String {
    match label.is_empty() {
        true => format!("{EVAL_WORKSPACE_PREFIX}{arm}-{repeat}"),
        false => format!("{EVAL_WORKSPACE_PREFIX}{label}-{arm}-{repeat}"),
    }
}

/// Directory this arm and repeat run in, under the eval root.
pub fn arm_workspace_path(eval_root: &Path, label: &RunLabel, arm: Arm, repeat: u32) -> PathBuf {
    eval_root.join(arm_workspace_name(label, arm, repeat))
}

/// Database name for this arm and repeat.
///
/// It must be what the gateway derives from the same workspace, or a browsed
/// arm opens an empty one. See `lucidos_gateway::postgres::database_name`, the
/// other side of the contract, which this crate cannot call: ADR 0014 §1 keeps
/// the gateway free of engine and harness dependencies alike.
pub fn arm_database_name(label: &RunLabel, arm: Arm, repeat: u32) -> String {
    format!("lucidos_{}", arm_workspace_name(label, arm, repeat))
}

/// Compose the arm's connection string from the base ADR 0087 pins.
///
/// The base carries host, port and credentials and never a database name, so
/// one setting serves every arm and repeat. A trailing slash on the base is
/// tolerated because an operator will paste one.
pub fn arm_database_url(pg_base: &str, label: &RunLabel, arm: Arm, repeat: u32) -> String {
    format!(
        "{}/{}",
        pg_base.trim_end_matches('/'),
        arm_database_name(label, arm, repeat)
    )
}

/// One seeded row, flattened to the pair the digest hashes.
///
/// `key` is whatever identifies the row inside its table, and `value` is its
/// remaining columns rendered in a fixed order. Both are opaque to the digest.
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

/// Hash the seeded rows into one hex digest.
///
/// Sorted first, so the answer does not depend on the order Postgres chose. The
/// separators are control characters, so no value can forge a row boundary.
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

/// Read the three seeded tables, excluding the permitted differences.
///
/// The excluded keys are the rows an arm may differ on, and there are three:
/// the mode's own flag and the two numbers beside it. They are filtered in SQL,
/// not by a text scan of a dump.
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
        "SELECT id, label, provider, sort_order::text AS sort_order, source, \
         enabled::text AS enabled FROM models",
    )
    .fetch_all(pool)
    .await?;
    for row in models {
        let id: String = row.try_get("id")?;
        let mut value = String::new();
        for column in ["label", "provider", "sort_order", "source", "enabled"] {
            let cell: String = row.try_get(column)?;
            value.push_str(&cell);
            value.push('\u{1d}');
        }
        rows.push(SeedRow::new("models", id, value));
    }

    Ok(rows)
}

/// Hash every file under a workspace's `data/` tree, path and content both.
///
/// Paths are relative to `data/` and sorted, so two trees agree only when they
/// hold the same files with the same bytes.
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

/// The pair of digests describing one seeded arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedDigest {
    pub db: String,
    pub fs: String,
}

/// Fail the run when the arms were not seeded identically (I1).
///
/// Called before the first prompt of a repeat. A difference here is an
/// uncontrolled variable, so it aborts rather than being recorded.
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

/// Copy the checked-in fixture tree over a workspace's `data/`, clearing first.
///
/// Clearing is what makes a re-seed reproducible: a file the previous run left
/// behind would otherwise ride into the digest. The workspace must already have
/// passed [`checked_eval_workspace`].
pub fn install_fixture_tree(workspace: &Path, fixture_root: &Path) -> Fallible<()> {
    checked_eval_workspace(workspace)?;
    let data = workspace.join("data");
    if data.exists() {
        std::fs::remove_dir_all(&data)?;
    }
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

/// A booted engine and the port it answers on.
///
/// Dropping this does not stop the process. Call [`stop_engine`], so a failure
/// path can decide whether to leave the engine up for inspection.
pub struct BootedEngine {
    pub child: std::process::Child,
    pub base_url: String,
    /// Where [`stop_engine`] removes the pidfile it wrote.
    workspace: PathBuf,
}

/// Whether the arm engines serve https, and with which certificate.
///
/// The scheme has to match what a dev gateway decided at ITS boot. A gateway
/// started with certificates probes and proxies https once and for the whole
/// process, so an arm serving plain http is unreachable through `/eval-lean-1/`.
/// This mirrors `detect_tls` in `scripts/lib/workspace.sh`, and is the one place
/// the scheme is decided: the health check, the driver's base URL and the
/// spawned engine all read it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineTls {
    certificate: Option<(PathBuf, PathBuf)>,
}

impl EngineTls {
    /// Read `LUCIDOS_EVAL_ENGINE_TLS_CERT` and `LUCIDOS_EVAL_ENGINE_TLS_KEY`.
    ///
    /// `scripts/eval-context-mode.sh` defaults both to the checkout's `.certs/`
    /// when they exist there, so the harness follows the dev stack by default.
    pub fn from_env() -> Fallible<EngineTls> {
        Self::resolve(
            std::env::var("LUCIDOS_EVAL_ENGINE_TLS_CERT").ok(),
            std::env::var("LUCIDOS_EVAL_ENGINE_TLS_KEY").ok(),
        )
    }

    /// The rule itself, taking the two values so a test can supply them.
    ///
    /// Exactly one set is refused rather than read as off. Reading it as plain
    /// http produces an arm the gateway cannot reach, hours into a run that
    /// costs real money.
    fn resolve(cert: Option<String>, key: Option<String>) -> Fallible<EngineTls> {
        let present = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        match (present(cert), present(key)) {
            (Some(cert), Some(key)) => Ok(EngineTls {
                certificate: Some((PathBuf::from(cert), PathBuf::from(key))),
            }),
            (None, None) => Ok(EngineTls::default()),
            (cert, _) => Err(format!(
                "LUCIDOS_EVAL_ENGINE_TLS_{} is set and the other is not. The arm engines serve \
                 one scheme, so set both or neither.",
                match cert.is_some() {
                    true => "CERT",
                    false => "KEY",
                }
            )
            .into()),
        }
    }

    /// What the arm engines serve, and what every client of them must speak.
    pub fn scheme(&self) -> &'static str {
        match self.certificate.is_some() {
            true => "https",
            false => "http",
        }
    }

    /// The engine's own `LUCIDOS_TLS_*` pair. Empty clears an inherited one, so
    /// a plain-http arm stays plain even under a shell that exported certs.
    fn engine_env(&self) -> [(&'static str, String); 2] {
        let (cert, key) = match &self.certificate {
            Some((cert, key)) => (cert.to_string_lossy(), key.to_string_lossy()),
            None => (std::borrow::Cow::from(""), std::borrow::Cow::from("")),
        };
        [
            ("LUCIDOS_TLS_CERT", cert.into_owned()),
            ("LUCIDOS_TLS_KEY", key.into_owned()),
        ]
    }
}

/// Drop and recreate this arm's database.
///
/// Destructive, and gated on the workspace name having passed
/// [`checked_eval_workspace`] first. The caller owns that order.
pub fn recreate_database(pg_base: &str, database: &str) -> Fallible<()> {
    let admin = format!("{}/postgres", pg_base.trim_end_matches('/'));
    let (drop, create) = recreate_statements(database);
    psql(&admin, &["-c", &drop, "-c", &create])
}

/// The two statements [`recreate_database`] runs, so a test can read them.
///
/// The name carries hyphens, because the gateway derives it from a slug and a
/// slug is `[a-z0-9-]`. Unquoted, Postgres parses `lucidos_eval-lean-1` as
/// subtraction and fails at the first hyphen.
fn recreate_statements(database: &str) -> (String, String) {
    let ident = quote_ident(database);
    (
        format!("DROP DATABASE IF EXISTS {ident} WITH (FORCE)"),
        format!("CREATE DATABASE {ident}"),
    )
}

/// Quote a Postgres identifier, doubling any embedded quote.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Bring the schema up by booting the engine once and stopping it again.
///
/// The specification's `--migrate-and-exit` does not exist, and adding it would
/// be an engine change the plan lists as a non-goal. Boot is the only thing
/// that runs the migration chain, so boot is what the seed uses.
///
/// On a throwaway port, deliberately. This boot exists to run migrations and
/// dies seconds later, so it must not sit on the port the gateway routes to.
pub fn migrate_by_booting(
    engine_bin: &Path,
    workspace: &Path,
    database_url: &str,
    tls: &EngineTls,
) -> Fallible<()> {
    // No capture: this boot exists to run migrations and never calls a model.
    let engine = boot_engine(
        engine_bin,
        workspace,
        database_url,
        free_port()?,
        tls,
        false,
    )?;
    stop_engine(engine)
}

/// Start an engine for one arm on `port` and wait until it answers.
///
/// `port` is the one the gateway registry holds for this arm. The gateway then
/// adopts this engine on the first proxy hit, rather than spawning a second one
/// against the same database. See [`crate::gateway::registered_port`].
///
/// The pidfile it writes is the same one `lucidos_gateway::stack::spawn_engine`
/// writes, at the path the picker's Stop and the gateway's stale-engine reclaim
/// both read.
pub fn boot_engine(
    engine_bin: &Path,
    workspace: &Path,
    database_url: &str,
    port: u16,
    tls: &EngineTls,
    full_capture: bool,
) -> Fallible<BootedEngine> {
    checked_eval_workspace(workspace)?;
    let child = std::process::Command::new(engine_bin)
        .envs(arm_engine_env(
            workspace,
            database_url,
            port,
            tls,
            full_capture,
        ))
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", engine_bin.display()))?;
    write_pidfile(workspace, child.id());
    let base_url = format!("{}://127.0.0.1:{port}", tls.scheme());
    let mut child = child;
    if let Err(error) = wait_for_health(&mut child, &base_url, Duration::from_secs(120)) {
        // Kill it here or it outlives the failed seed, holding a port and a
        // database connection the next attempt needs.
        let _ = child.kill();
        let _ = child.wait();
        remove_pidfile(workspace);
        return Err(error);
    }
    Ok(BootedEngine {
        child,
        base_url,
        workspace: workspace.to_path_buf(),
    })
}

/// Everything one arm's engine is launched with.
///
/// Split out of [`boot_engine`] so a test can read the set without spawning a
/// process. Both arms boot through that one function, so a pair added here
/// reaches control and lean alike.
fn arm_engine_env(
    workspace: &Path,
    database_url: &str,
    port: u16,
    tls: &EngineTls,
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
    ];
    env.extend(
        tls.engine_env()
            .into_iter()
            .map(|(key, value)| (key, OsString::from(value))),
    );
    if full_capture {
        env.push((FULL_CAPTURE_ENV, OsString::from("1")));
    }
    env
}

/// Makes the arm's engine persist whole `ContextCaptured` bodies.
///
/// A capture normally truncates every section head and tail at 8,000 chars, so
/// its `content_chars` is honest and its body is not. That is fine for the
/// Context Viewer and useless for replaying a 137,000-char message array, which
/// is the region a context benchmark is about.
///
/// The engine spells the same name in `engine::eval_capture::FULL_CAPTURE_ENV`,
/// and the name IS the contract. Set on the arms alone, exactly as the query
/// classifier pin is. See ADR 0110 decision 12.
pub const FULL_CAPTURE_ENV: &str = "LUCIDOS_EVAL_FULL_CAPTURE";

/// Pins what the engine's query classifier answers, instead of asking an LLM.
///
/// The classifier decides `needs_memory` through a Flash call, so the two arms
/// disagree from one run to the next. A disagreement voids the whole task pair
/// ([`crate::analyse::retrieval_disagreed`]), because such a pair measures the
/// classifier rather than the flag. It cost the last run its two most valuable
/// tasks. Pinning both arms removes the confound.
const FORCE_QUERY_CLASSIFICATION_ENV: &str = "LUCIDOS_FORCE_QUERY_CLASSIFICATION";

/// `all`, deliberately, and never `none`.
///
/// Long-term memory is one of the three sections the curated context mode can
/// drop (`MEMORY_SECTION`). Retrieving it on every turn is what gives the mode
/// something to drop, so `all` exercises the thing under test. `none` would
/// hide it, and the run would measure a turn the mode barely touches.
const FORCE_QUERY_CLASSIFICATION_VALUE: &str = "all";

/// Stop an engine and wait for it to go.
pub fn stop_engine(mut engine: BootedEngine) -> Fallible<()> {
    engine.child.kill()?;
    engine.child.wait()?;
    // The process is gone, so the file would only offer the gateway's reclaim a
    // pid the operating system is free to hand to somebody else.
    remove_pidfile(&engine.workspace);
    Ok(())
}

/// Where a workspace's engine records its pid, for the gateway to read.
fn pidfile(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos/engine.pid")
}

/// Record this engine's pid. Not fatal: the engine is already up and serving,
/// and only the picker's Stop and the gateway's reclaim lose sight of it.
fn write_pidfile(workspace: &Path, pid: u32) {
    let path = pidfile(workspace);
    let written = std::fs::create_dir_all(workspace.join(".lucidos"))
        .and_then(|()| std::fs::write(&path, pid.to_string()));
    if let Err(e) = written {
        eprintln!(
            "[eval] could not write {}: {e}. The picker's Stop and the gateway's stale-engine \
             reclaim will not see this engine.",
            path.display()
        );
    }
}

fn remove_pidfile(workspace: &Path) {
    let _ = std::fs::remove_file(pidfile(workspace));
}

/// Poll `base_url` until OUR engine answers, or the deadline passes.
///
/// `-k` because the loopback certificate is self-signed, matching the driver's
/// own `danger_accept_invalid_certs`. The scheme is already in `base_url`, from
/// the single [`EngineTls`] rule.
///
/// **A healthy port is not proof, so `child` is checked too.** The arm now
/// boots on the port the gateway routes to, and the gateway may already have
/// lazy-started an engine of its own there. Ours would then fail to bind and
/// exit, while the probe passes against the gateway's. The harness would drive
/// a thread against an engine it does not own, and `stop_engine` would kill an
/// already-dead child and leave that one running.
fn wait_for_health(
    child: &mut std::process::Child,
    base_url: &str,
    timeout: Duration,
) -> Fallible<()> {
    let deadline = std::time::Instant::now() + timeout;
    let url = format!("{base_url}/api/v1/health");
    loop {
        // Before the probe, so an engine that exited is reported as that rather
        // than as somebody else's healthy port.
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "engine_exited_during_boot: the engine left with {status} before answering \
                 {url}. Another process may already hold that port: stop the workspace in \
                 the picker, or check the engine's own output."
            )
            .into());
        }
        let healthy = std::process::Command::new("curl")
            .args(["-skf", "-o", "/dev/null", "--max-time", "2", &url])
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

/// A port nothing is listening on right now.
pub fn free_port() -> Fallible<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Wait until `port` accepts a bind, answering whether it came free in time.
///
/// The gateway's stop signals its engine and reaps it on another thread. The
/// call therefore returns while the port is still held, and a graceful drain
/// can take seconds. Binding straight away would fail, and the arm would then
/// boot on no port at all.
///
/// A held port fails a bind even with `SO_REUSEADDR`, which is what makes this
/// probe honest: only `TIME_WAIT` is excused, never a live listener.
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

/// The pins `seed.sql` needs as psql variables.
pub struct SeedPins<'a> {
    pub model: &'a str,
    pub model_label: &'a str,
    pub model_provider: &'a str,
    pub reasoning_effort: &'a str,
    /// The context window to declare on the seeded model row, or `None` to let
    /// the engine infer it from the model id.
    ///
    /// This is the budget-pressure knob (ADR 0110 decision 9). The engine
    /// derives its char budget from the registry row, so declaring a smaller
    /// window here makes it trim earlier and changes no engine code.
    pub context_window: Option<i64>,
}

/// Apply `seed.sql` to a freshly migrated database.
///
/// `context_window` is passed as the literal `NULL` when unset, so the row's
/// column is genuinely null and `context_window_for` falls back to the prefix
/// map. Quoting it as an empty string would insert one instead.
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
            &format!("context_window={window}"),
            "-f",
            &seed_sql.to_string_lossy(),
        ],
    )
}

/// Write the preference rows that make this workspace an arm.
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
    let output = std::process::Command::new("psql")
        .arg(database_url)
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

    /// I5.
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

    /// I5's other half. A correctly named symlink aimed at a live workspace
    /// would pass a name check, and clearing the data tree would follow it.
    #[test]
    fn a_correctly_named_symlink_is_refused() {
        let root = std::env::temp_dir().join(format!(
            "lucidos-eval-symlink-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let target = root.join("somebody-elses-workspace");
        let link = root.join("eval-lean-1");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = checked_eval_workspace(&link).unwrap_err().to_string();
        assert!(err.contains("it is a symlink"), "got: {err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_real_directory_with_the_prefix_is_still_accepted() {
        let root = std::env::temp_dir().join(format!(
            "lucidos-eval-real-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let workspace = root.join("eval-lean-1");
        std::fs::create_dir_all(&workspace).unwrap();
        checked_eval_workspace(&workspace).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn opus() -> RunLabel {
        RunLabel::derive("claude-opus-5@default")
    }

    #[test]
    fn the_label_arm_and_repeat_name_the_workspace_and_the_database() {
        let root = Path::new("/tmp/eval-root");
        let label = opus();
        assert_eq!(
            arm_workspace_path(root, &label, Arm::Lean, 3),
            root.join(format!("eval-{label}-lean-3"))
        );
        assert_eq!(
            arm_database_name(&label, Arm::Control, 2),
            format!("lucidos_eval-{label}-control-2")
        );
        assert_eq!(
            arm_database_url("postgres://u:p@localhost:5438/", &label, Arm::Lean, 1),
            format!("postgres://u:p@localhost:5438/lucidos_eval-{label}-lean-1")
        );
    }

    /// The whole point of the label: two providers, one arm, one repeat, and
    /// nothing shared. Without it both runs create `lucidos_eval-lean-1`.
    #[test]
    fn two_models_never_share_a_workspace_or_a_database() {
        let anthropic = RunLabel::derive("claude-opus-5@default");
        let openai = RunLabel::derive("gpt-5.6-sol");
        assert_ne!(
            arm_workspace_name(&anthropic, Arm::Lean, 1),
            arm_workspace_name(&openai, Arm::Lean, 1)
        );
        assert_ne!(
            arm_database_name(&anthropic, Arm::Lean, 1),
            arm_database_name(&openai, Arm::Lean, 1)
        );
    }

    /// A model id is not a slug, so every separator it carries has to come
    /// out. The stem still has to read like the id it came from: an operator
    /// reads it off a run log to open the database by hand.
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

    /// Sanitising is lossy, so it merges ids exactly as truncating does.
    ///
    /// These pairs differ only in punctuation or case, so every one of them
    /// reduces to a single stem. Two such runs sharing a database is the
    /// failure the label exists to prevent, so the digest covers the untouched
    /// source rather than the stem.
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
                arm_database_name(&first, Arm::Lean, 1),
                arm_database_name(&second, Arm::Lean, 1)
            );
        }
    }

    /// I5 refuses any workspace that does not carry the prefix, and it is what
    /// gates dropping a database and clearing a data tree.
    #[test]
    fn a_labelled_workspace_still_carries_the_eval_prefix() {
        let name = arm_workspace_name(&opus(), Arm::Control, 7);
        assert!(name.starts_with(EVAL_WORKSPACE_PREFIX), "{name}");
        assert!(name.ends_with("-control-7"), "{name}");
    }

    /// A Postgres identifier holds 63 bytes, and this one already needs
    /// quoting for its hyphens. The fixed part is computed rather than assumed,
    /// so `MAX_LABEL_BYTES` cannot drift away from the real ceiling.
    #[test]
    fn the_longest_possible_name_fits_a_postgres_identifier() {
        let longest_label = RunLabel("z".repeat(MAX_LABEL_BYTES));
        let name = arm_database_name(&longest_label, Arm::Control, u32::MAX);
        assert!(name.len() <= 63, "{} bytes: {name}", name.len());

        // And the label really is the largest piece that can grow, so one more
        // byte would have spent the last of the headroom.
        let over = RunLabel("z".repeat(MAX_LABEL_BYTES + 1));
        assert_eq!(
            arm_database_name(&over, Arm::Control, u32::MAX).len(),
            64,
            "the ceiling is exactly one byte above the longest legal label"
        );
    }

    /// The same guarantee where TRUNCATION is what would have merged them.
    ///
    /// These two ids agree far past the stem cap, so the readable halves come
    /// out identical and only the digest separates them.
    #[test]
    fn two_long_model_ids_sharing_a_prefix_get_different_labels() {
        let first = RunLabel::derive("some-very-long-provider/experimental-model-alpha-2026");
        let second = RunLabel::derive("some-very-long-provider/experimental-model-beta-2026");
        assert_ne!(first, second, "{first} collided with {second}");
        assert!(first.as_str().len() <= MAX_LABEL_BYTES, "{first}");
        assert!(second.as_str().len() <= MAX_LABEL_BYTES, "{second}");
        assert_ne!(
            arm_database_name(&first, Arm::Lean, 1),
            arm_database_name(&second, Arm::Lean, 1)
        );
    }

    /// Deriving twice gives the same label, or a resume looks for databases
    /// that were never created.
    #[test]
    fn a_derived_label_is_stable_across_calls() {
        let id = "some-very-long-provider/experimental-model-alpha-2026";
        assert_eq!(RunLabel::derive(id), RunLabel::derive(id));
    }

    /// A recorded label is read back as itself, so a post-run command opens the
    /// database the run actually wrote. Re-deriving would digest the digest and
    /// name a database nothing ever created.
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

    /// Scoring a Sol run from a shell pinned to Opus must not open the Opus
    /// arms. That is what reading the label off the file rather than off the
    /// environment buys, so the two resolutions have to stay distinguishable.
    #[test]
    fn a_recorded_label_beats_whatever_the_shell_is_pinned_to() {
        let recorded = RunLabel::recorded("gpt-5-6-sol");
        let ambient = RunLabel::derive("claude-opus-5@default");
        assert_ne!(
            arm_database_name(&recorded, Arm::Lean, 1),
            arm_database_name(&ambient, Arm::Lean, 1)
        );
        assert_eq!(
            arm_database_name(&recorded, Arm::Lean, 1),
            "lucidos_eval-gpt-5-6-sol-lean-1"
        );
    }

    /// A run recorded before the label existed named its workspaces without
    /// one. Scoring or replaying it has to find those names, not new ones.
    #[test]
    fn an_unlabelled_run_keeps_the_name_it_was_created_with() {
        let none = RunLabel::recorded("");
        assert_eq!(arm_workspace_name(&none, Arm::Lean, 1), "eval-lean-1");
        assert_eq!(
            arm_database_name(&none, Arm::Lean, 1),
            "lucidos_eval-lean-1"
        );
        // And it still satisfies I5, which is what allows the harness to touch
        // the directory at all.
        assert!(arm_workspace_name(&none, Arm::Control, 2).starts_with(EVAL_WORKSPACE_PREFIX));
    }

    /// An id with nothing alphanumeric in it still has to name a database.
    #[test]
    fn an_id_that_sanitises_to_nothing_still_yields_a_label() {
        let label = RunLabel::derive("///");
        assert!(!label.as_str().is_empty());
        assert!(arm_database_name(&label, Arm::Lean, 1).len() <= 63);
    }

    /// The contract with the gateway, spelled out because it cannot be called.
    ///
    /// `lucidos_gateway::postgres::database_name` returns `lucidos_<slug>`, and
    /// the slug is `registry::slugify` of the directory basename. This crate
    /// must not depend on that one (ADR 0014 §1), so the agreement is a literal
    /// on each side. The gateway's half is
    /// `database_name_preserves_slug_shape_under_lucidos_prefix`.
    ///
    /// Get this wrong and nothing fails loudly. The harness seeds one database
    /// and the gateway boots the arm against another, empty one.
    #[test]
    fn the_database_name_is_what_the_gateway_derives_from_the_slug() {
        // A recorded label, so the literals below stay readable. A derived one
        // ends in a digest, which this test is not about.
        let label = RunLabel::recorded("m");
        assert_eq!(arm_workspace_name(&label, Arm::Lean, 1), "eval-m-lean-1");
        assert_eq!(
            arm_database_name(&label, Arm::Lean, 1),
            "lucidos_eval-m-lean-1"
        );
        // Underscores were the old shape, and the gateway can never produce
        // one: a slug is `[a-z0-9-]`. Only the `lucidos_` prefix carries one.
        assert!(!arm_workspace_name(&label, Arm::Lean, 1).contains('_'));
    }

    /// The hyphens the gateway's shape forces have to survive into SQL.
    #[test]
    fn the_hyphenated_database_name_is_quoted_in_every_statement() {
        let name = arm_database_name(&RunLabel::recorded("m"), Arm::Lean, 1);
        let (drop, create) = recreate_statements(&name);
        assert_eq!(
            drop,
            "DROP DATABASE IF EXISTS \"lucidos_eval-m-lean-1\" WITH (FORCE)"
        );
        assert_eq!(create, "CREATE DATABASE \"lucidos_eval-m-lean-1\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    // ── One scheme, decided once ─────────────────────────────────────────────

    fn tls(cert: Option<&str>, key: Option<&str>) -> Fallible<EngineTls> {
        EngineTls::resolve(cert.map(str::to_string), key.map(str::to_string))
    }

    #[test]
    fn no_certificate_means_the_arms_serve_plain_http() {
        assert_eq!(tls(None, None).unwrap().scheme(), "http");
        // A shell that exported a certificate does not leak into the arm: the
        // engine's own pair is cleared rather than left inherited.
        assert_eq!(
            tls(None, None).unwrap().engine_env(),
            [
                ("LUCIDOS_TLS_CERT", String::new()),
                ("LUCIDOS_TLS_KEY", String::new())
            ]
        );
        // Blank is the same as unset. The launch script exports an empty value
        // when the checkout has no `.certs/`.
        assert_eq!(tls(Some("  "), Some("")).unwrap().scheme(), "http");
    }

    #[test]
    fn a_certificate_pair_makes_the_arms_serve_https() {
        let with = tls(Some("/c/cert.pem"), Some("/c/key.pem")).unwrap();
        assert_eq!(with.scheme(), "https");
        assert_eq!(
            with.engine_env(),
            [
                ("LUCIDOS_TLS_CERT", "/c/cert.pem".to_string()),
                ("LUCIDOS_TLS_KEY", "/c/key.pem".to_string())
            ]
        );
    }

    /// Half a pair is refused rather than read as off.
    ///
    /// A dev gateway decides `engine_tls` once at its own boot and probes that
    /// scheme forever after. An arm that quietly fell back to http would be
    /// unreachable through `/eval-lean-1/`, discovered hours into a paid run.
    #[test]
    fn exactly_one_tls_variable_is_a_refusal_naming_the_one_that_is_set() {
        let err = tls(Some("/c/cert.pem"), None).unwrap_err().to_string();
        assert!(err.contains("LUCIDOS_EVAL_ENGINE_TLS_CERT"), "{err}");
        assert!(err.contains("set both or neither"), "{err}");
        let err = tls(None, Some("/c/key.pem")).unwrap_err().to_string();
        assert!(err.contains("LUCIDOS_EVAL_ENGINE_TLS_KEY"), "{err}");
    }

    // ── The classifier pin both arms boot with ───────────────────────────────

    fn arm_env(tls: &EngineTls) -> Vec<(&'static str, OsString)> {
        arm_engine_env(
            Path::new("/tmp/eval-lean-1"),
            "postgres://localhost/lucidos_eval-lean-1",
            3210,
            tls,
            true,
        )
    }

    fn value_of<'a>(env: &'a [(&'static str, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// The whole point of the pin: neither arm asks the LLM, so neither can
    /// answer differently and void the pair.
    #[test]
    fn every_arm_boots_with_the_classifier_pinned_to_all() {
        for engine_tls in [
            tls(None, None).unwrap(),
            tls(Some("/c/c"), Some("/c/k")).unwrap(),
        ] {
            let env = arm_env(&engine_tls);
            assert_eq!(
                value_of(&env, "LUCIDOS_FORCE_QUERY_CLASSIFICATION"),
                Some(&OsString::from("all")),
                "the pin is missing from an arm's environment"
            );
        }
    }

    /// `none` would drop the memory section the curated mode is measured on.
    #[test]
    fn the_pin_is_all_and_not_none() {
        assert_eq!(FORCE_QUERY_CLASSIFICATION_VALUE, "all");
    }

    /// The pin is added to the existing set, not in place of it. A boot that
    /// lost `DATABASE_URL` would come up against the wrong database.
    #[test]
    fn the_pin_did_not_displace_what_the_engine_already_needed() {
        let env = arm_env(&tls(Some("/c/cert.pem"), Some("/c/key.pem")).unwrap());
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
        assert_eq!(
            value_of(&env, "LUCIDOS_TLS_CERT"),
            Some(&OsString::from("/c/cert.pem"))
        );
        assert_eq!(
            value_of(&env, "LUCIDOS_TLS_KEY"),
            Some(&OsString::from("/c/key.pem"))
        );
    }

    /// A parallel OpenAI and Vertex run needs both keys live at once.
    ///
    /// Each arm's database is created empty, so its `credentials` table has no
    /// row and every provider falls back to its environment variable. This set
    /// is passed to `Command::envs`, which ADDS to the inherited environment,
    /// so both keys reach both arms. Naming one here would be the mistake: the
    /// harness would then decide which provider an arm can reach.
    #[test]
    fn the_arm_environment_names_no_provider_credential() {
        let env = arm_env(&tls(None, None).unwrap());
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
        // And the seed writes no credential row either, which is what makes the
        // env fallback the only route.
        assert!(!SEEDED_TABLES.contains(&"credentials"));
    }

    #[test]
    fn the_digest_ignores_the_order_rows_arrived_in() {
        let forward = rows();
        let mut backward = rows();
        backward.reverse();
        assert_eq!(digest_rows(&forward), digest_rows(&backward));
    }

    /// I1: mutating one seeded row makes the comparison fail.
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

    /// I1, the other half: identical seeding passes, and the arm's own
    /// preference row is not part of what is compared.
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

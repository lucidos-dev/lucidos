use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::{RecordedFile, StartupInstructions};

const LEDGER_FILE: &str = ".lucidos/cc-startup-instructions-ledger.json";

const ENTRIES_KEPT: usize = 256;

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyState {
    Verified,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Verified,
    Failed,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct StartupLedger {
    #[serde(default)]
    verified: Vec<String>,
    #[serde(default)]
    failed: Vec<String>,
    /// INVARIANT: Sessions that started with their files in the system prompt. Claude Code
    /// freezes that prompt, so such a session must resume the same way.
    #[serde(default)]
    moved_sessions: Vec<String>,
}

/// INVARIANT: One writer at a time, so two sessions finishing together never lose a key.
static LEDGER_WRITE: Mutex<()> = Mutex::new(());

impl StartupLedger {
    fn path(workspace: &Path) -> PathBuf {
        workspace.join(LEDGER_FILE)
    }

    fn load(workspace: &Path) -> Self {
        std::fs::read_to_string(Self::path(workspace))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn state_of(workspace: &Path, key: &str) -> KeyState {
        let ledger = Self::load(workspace);
        if ledger.verified.iter().any(|k| k == key) {
            KeyState::Verified
        } else if ledger.failed.iter().any(|k| k == key) {
            KeyState::Failed
        } else {
            KeyState::Unknown
        }
    }

    pub(crate) fn session_moved(workspace: &Path, session_id: &str) -> bool {
        Self::load(workspace)
            .moved_sessions
            .iter()
            .any(|id| id == session_id)
    }

    pub(crate) fn record(workspace: &Path, key: &str, verdict: Verdict) {
        Self::add(workspace, key, |ledger| match verdict {
            Verdict::Verified => &mut ledger.verified,
            Verdict::Failed => &mut ledger.failed,
        });
    }

    pub(crate) fn record_moved_session(workspace: &Path, session_id: &str) {
        Self::add(workspace, session_id, |ledger| &mut ledger.moved_sessions);
    }

    fn add(workspace: &Path, entry: &str, pick: impl FnOnce(&mut Self) -> &mut Vec<String>) {
        let _guard = LEDGER_WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut ledger = Self::load(workspace);
        let list = pick(&mut ledger);
        if list.iter().any(|known| known == entry) {
            return;
        }
        list.push(entry.to_string());
        if list.len() > ENTRIES_KEPT {
            list.remove(0);
        }
        let path = Self::path(workspace);
        let written = serde_json::to_string(&ledger)
            .map_err(std::io::Error::other)
            .and_then(|body| {
                std::fs::create_dir_all(path.parent().unwrap_or(workspace))?;
                let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
                std::fs::write(&temp, body)?;
                std::fs::rename(temp, &path)
            });
        if let Err(e) = written {
            crate::log_legacy!(
                "[StartupInstructions] could not write the ledger {}: {}",
                path.display(),
                e
            );
        }
    }
}

pub(crate) enum StartupFollowUp {
    Check(StartupCheck),
    Moved { workspace: PathBuf },
}

impl StartupFollowUp {
    pub(crate) fn moved(workspace: &Path) -> Self {
        Self::Moved {
            workspace: workspace.to_path_buf(),
        }
    }

    /// INVARIANT: Do the follow-up. Blocking, so call it off the async workers.
    pub(crate) fn run(&self, session_id: &str) {
        match self {
            Self::Check(check) => check.run(session_id),
            Self::Moved { workspace } => StartupLedger::record_moved_session(workspace, session_id),
        }
    }
}

pub(crate) struct StartupCheck {
    workspace: PathBuf,
    key: String,
    expected: StartupInstructions,
    transcript_dir: PathBuf,
}

impl StartupCheck {
    pub(super) fn new(
        workspace: &Path,
        key: String,
        expected: StartupInstructions,
        config_dir: &Path,
    ) -> Self {
        let transcript_dir = config_dir
            .join("projects")
            .join(transcript_dir_name(expected.cwd()));
        Self {
            workspace: workspace.to_path_buf(),
            key,
            expected,
            transcript_dir,
        }
    }

    /// INVARIANT: Compare the session's record with the mirror and write the verdict. A
    /// transcript that is missing or unreadable is no verdict: the key stays
    /// unknown. Blocking, so call it off the async workers.
    pub(crate) fn run(&self, session_id: &str) {
        let path = self.transcript_dir.join(format!("{session_id}.jsonl"));
        let recorded = match last_recorded_instructions(&path) {
            Ok(Some(recorded)) => recorded,
            Ok(None) => {
                crate::log_legacy!(
                    "[StartupInstructions] {} has no instructions record. Key stays unverified.",
                    path.display()
                );
                return;
            }
            Err(e) => {
                crate::log_legacy!(
                    "[StartupInstructions] cannot read {}: {}. Key stays unverified.",
                    path.display(),
                    e
                );
                return;
            }
        };
        match self.expected.first_difference(&recorded) {
            None => {
                StartupLedger::record(&self.workspace, &self.key, Verdict::Verified);
                crate::log_legacy!(
                    "[StartupInstructions] verified a key: Claude Code loaded exactly the \
                     mirror's {} files",
                    recorded.len()
                );
            }
            Some(difference) => {
                StartupLedger::record(&self.workspace, &self.key, Verdict::Failed);
                crate::log_legacy!(
                    "[StartupInstructions] the mirror differs from Claude Code, so these \
                     files stay unmoved: {}",
                    difference
                );
            }
        }
    }
}

fn transcript_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn last_recorded_instructions(path: &Path) -> std::io::Result<Option<Vec<RecordedFile>>> {
    let file = std::fs::File::open(path)?;
    let mut last = None;
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if !line.contains("\"instructions\"") {
            continue;
        }
        if let Some(recorded) = recorded_instructions(&line) {
            last = Some(recorded);
        }
    }
    Ok(last)
}

fn recorded_instructions(line: &str) -> Option<Vec<RecordedFile>> {
    let record: serde_json::Value = serde_json::from_str(line).ok()?;
    let attachment = record
        .get("attachment")
        .filter(|_| record["type"] == "attachment")?;
    if attachment["type"] != "instructions" {
        return None;
    }
    attachment["files"]
        .as_array()?
        .iter()
        .map(|file| {
            Some(RecordedFile {
                kind: file["type"].as_str()?.to_string(),
                path: file["path"].as_str()?.to_string(),
                content: file["content"].as_str()?.to_string(),
            })
        })
        .collect()
}

type VersionKey = (PathBuf, Option<SystemTime>);

static VERSIONS: Mutex<Option<HashMap<VersionKey, Option<String>>>> = Mutex::new(None);

pub(crate) fn claude_version(binary: &OsStr) -> Option<String> {
    let path = crate::runtime::spawn_env::find_on_path(binary, std::env::var_os("PATH").as_deref())
        .or_else(|| Path::new(binary).is_file().then(|| PathBuf::from(binary)))?;
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let key = (path.clone(), modified);
    let cached = VERSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|cache| cache.get(&key).cloned());
    if let Some(version) = cached {
        return version;
    }
    // INVARIANT: The probe runs unlocked, so a slow binary never stalls other spawns. Two
    // first spawns may both probe, and the answers agree.
    let version = probe_version(&path);
    VERSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key, version.clone());
    version
}

fn probe_version(binary: &Path) -> Option<String> {
    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + VERSION_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                crate::log_legacy!(
                    "[StartupInstructions] {} --version did not answer in {}s",
                    binary.display(),
                    VERSION_PROBE_TIMEOUT.as_secs()
                );
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout?, &mut out).ok()?;
    let version = out.trim().to_string();
    (status.success() && !version.is_empty()).then_some(version)
}

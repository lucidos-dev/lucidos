//! Durable inputs for scoring and reporting after an eval arm is removed.
//!
//! One immutable file publishes the database observations and data tree together.
//! Readers reject corrupt or mismatched snapshots instead of falling back to a
//! potentially reused live workspace. No engine or model is invoked here.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{arm::Arm, calls, replay, Fallible, ThreadWorld};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    schema: u32,
    run_id: String,
    label: String,
    arm: Arm,
    repeat: u32,
    pub threads: BTreeMap<Uuid, ThreadInputs>,
    pub calls: Vec<calls::PurposeCalls>,
    pub tree: Option<calls::TreeNodes>,
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadInputs {
    pub world: ThreadWorld,
    pub rounds: Vec<replay::Round>,
}

fn path(root: &Path, run: &str, arm: Arm, repeat: u32) -> Fallible<PathBuf> {
    if run.is_empty()
        || !run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid snapshot run identity".into());
    }
    Ok(root.join(format!("{run}-{arm}-{repeat}.snapshot.json")))
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

impl Snapshot {
    pub fn new(run: &str, label: &str, arm: Arm, repeat: u32, data: &Path) -> Fallible<Self> {
        let mut files = BTreeMap::new();
        collect(data, data, &mut files)?;
        Ok(Self {
            schema: 1,
            run_id: run.into(),
            label: label.into(),
            arm,
            repeat,
            threads: BTreeMap::new(),
            calls: Vec::new(),
            tree: None,
            files,
        })
    }

    pub fn read(
        root: &Path,
        run: &str,
        label: &str,
        arm: Arm,
        repeat: u32,
    ) -> Fallible<Option<Self>> {
        let bytes = match std::fs::read(path(root, run, arm, repeat)?) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let snapshot: Self = serde_json::from_slice(&bytes)?;
        if snapshot.schema != 1
            || snapshot.run_id != run
            || snapshot.label != label
            || snapshot.arm != arm
            || snapshot.repeat != repeat
            || snapshot.files.keys().any(|p| !safe_relative(p))
        {
            return Err("snapshot schema, identity or file path mismatch".into());
        }
        Ok(Some(snapshot))
    }

    pub fn publish(&self, root: &Path) -> Fallible<()> {
        std::fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path(root, &self.run_id, self.arm, self.repeat)?)?;
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }

    pub fn data_dir(&self) -> Fallible<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        for (relative, bytes) in &self.files {
            if !safe_relative(relative) {
                return Err("unsafe snapshot file path".into());
            }
            let file = dir.path().join(relative);
            std::fs::create_dir_all(file.parent().ok_or("snapshot file has no parent")?)?;
            std::fs::write(file, bytes)?;
        }
        Ok(dir)
    }
}

fn collect(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> Fallible<()> {
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err("snapshot data must be a directory, without symlinks".into());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &entry.path(), files)?;
        } else if kind.is_file() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("non-UTF8 snapshot path")?;
            if !safe_relative(relative) {
                return Err("non-portable snapshot path".into());
            }
            files.insert(relative.into(), std::fs::read(path)?);
        } else {
            return Err("snapshot data contains a symlink or special file; arm retained".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_inputs_survive_source_removal_and_refuse_replacement() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("artifacts")).unwrap();
        std::fs::write(source.path().join("artifacts/result.txt"), b"evidence").unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut snapshot = Snapshot::new("run", "label", Arm::LEAN, 1, source.path()).unwrap();
        let thread = Uuid::new_v4();
        snapshot.threads.insert(
            thread,
            ThreadInputs {
                world: ThreadWorld {
                    tool_calls: vec![crate::assertions::ToolCall {
                        sequence: 2,
                        name: "read_file".into(),
                        args: "{}".into(),
                    }],
                    events: vec![crate::assertions::EventRow {
                        sequence: 1,
                        event_type: "MessageReceived".into(),
                        payload: "{}".into(),
                        created: chrono::Utc::now(),
                    }],
                    round_two_sequence: Some(3),
                    followup_sequence: Some(4),
                    final_response: "delivered".into(),
                    questions: vec!["Which result?".into()],
                },
                rounds: vec![replay::Round {
                    round: 1,
                    capture: serde_json::json!({"trim_passes": [1]}),
                    after: vec![replay::Happened {
                        event_type: "ResponseGenerated".into(),
                        payload: serde_json::json!({"text": "delivered"}),
                    }],
                }],
            },
        );
        snapshot.calls.push(calls::PurposeCalls {
            purpose: "turn".into(),
            model: "test-model".into(),
            calls: 1,
            counts: Default::default(),
            usd: 0.25,
        });
        snapshot.tree = Some(calls::TreeNodes {
            written: 1,
            free: 2,
        });
        snapshot.publish(output.path()).unwrap();
        assert!(snapshot.publish(output.path()).is_err());
        source.close().unwrap();
        let restored = Snapshot::read(output.path(), "run", "label", Arm::LEAN, 1)
            .unwrap()
            .unwrap();
        let data = restored.data_dir().unwrap();
        assert_eq!(
            std::fs::read(data.path().join("artifacts/result.txt")).unwrap(),
            b"evidence"
        );
        let inputs = &restored.threads[&thread];
        assert_eq!(inputs.world.final_response, "delivered");
        assert_eq!(inputs.world.tool_calls[0].name, "read_file");
        assert_eq!(inputs.world.events[0].sequence, 1);
        assert_eq!(inputs.world.followup_sequence, Some(4));
        assert_eq!(inputs.rounds[0].after[0].payload["text"], "delivered");
        assert_eq!(restored.calls[0].usd, 0.25);
        assert_eq!(restored.tree.unwrap().free, 2);
        let context = crate::assertions::AssertionContext {
            data_dir: data.path(),
            final_response: &inputs.world.final_response,
            tool_calls: &inputs.world.tool_calls,
            events: &inputs.world.events,
            round_two_sequence: inputs.world.round_two_sequence,
            followup_sequence: inputs.world.followup_sequence,
        };
        assert!(crate::assertions::Assertion::FileExists {
            path: "artifacts/result.txt".into()
        }
        .evaluate(&context)
        .unwrap());
        assert!(Snapshot::read(output.path(), "run", "wrong", Arm::LEAN, 1).is_err());
    }

    #[test]
    fn corrupt_schema_and_paths_fail_closed() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut snapshot = Snapshot::new("run", "label", Arm::LEAN, 1, source.path()).unwrap();
        snapshot.files.insert("../escape".into(), vec![]);
        assert!(snapshot.data_dir().is_err());
        snapshot.publish(output.path()).unwrap();
        assert!(Snapshot::read(output.path(), "run", "label", Arm::LEAN, 1).is_err());
        let file = path(output.path(), "run", Arm::LEAN, 1).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        value["files"] = serde_json::json!({});
        value["schema"] = 2.into();
        std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Snapshot::read(output.path(), "run", "label", Arm::LEAN, 1).is_err());
        std::fs::write(&file, b"interrupted").unwrap();
        assert!(Snapshot::read(output.path(), "run", "label", Arm::LEAN, 1).is_err());
        assert!(Snapshot::read(output.path(), "../run", "label", Arm::LEAN, 1).is_err());
        assert!(
            Snapshot::read(output.path(), "missing", "label", Arm::LEAN, 1)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_sources_are_not_followed() {
        let source = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/outside", source.path().join("link")).unwrap();
        assert!(Snapshot::new("run", "label", Arm::LEAN, 1, source.path()).is_err());
    }
}

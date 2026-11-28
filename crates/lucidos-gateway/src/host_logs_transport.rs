use super::*;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

const DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);

fn target(recipient: &Recipient) -> String {
    let host = if recipient.host.contains(':') {
        format!("[{}]", recipient.host)
    } else {
        recipient.host.clone()
    };
    format!("{}@{host}", recipient.user)
}

fn command(recipient: &Recipient, cwd: &Path, max_file: u64) -> tokio::process::Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = tokio::process::Command::new("/usr/bin/sftp");
    cmd.env_clear()
        .args([
            "-F",
            "/dev/null",
            "-b",
            "-",
            "-oBatchMode=yes",
            "-oStrictHostKeyChecking=yes",
            "-oConnectTimeout=10",
            "-oConnectionAttempts=1",
            "-oProxyCommand=none",
            "-oProxyJump=none",
            "-oPermitLocalCommand=no",
            "-oClearAllForwardings=yes",
            "-oForwardAgent=no",
            "-oForwardX11=no",
            "-oControlMaster=no",
            "-oControlPath=none",
            "-oUpdateHostKeys=no",
        ])
        .arg(target(recipient))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(socket) = std::env::var_os("SSH_AUTH_SOCK") {
        cmd.env("SSH_AUTH_SOCK", socket);
    }
    cmd.as_std_mut().process_group(0);
    // A hostile remote cannot fill disk through a verification download.
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: max_file as _,
                rlim_max: max_file as _,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

async fn batch(recipient: &Recipient, cwd: &Path, script: &str, max_file: u64) -> Result<()> {
    let mut child = command(recipient, cwd, max_file).spawn()?;
    let _group = ProcessGroup(child.id().ok_or("SFTP did not start")?);
    let mut stdin = child.stdin.take().ok_or("SFTP input unavailable")?;
    stdin.write_all(script.as_bytes()).await?;
    drop(stdin);
    if !child.wait().await?.success() {
        return Err("SFTP failed; remote outcome may be incomplete".into());
    }
    Ok(())
}

trait Sftp {
    async fn run(
        &self,
        recipient: &Recipient,
        cwd: &Path,
        script: &str,
        max_file: u64,
    ) -> Result<()>;
}
struct SystemSftp;
impl Sftp for SystemSftp {
    async fn run(
        &self,
        recipient: &Recipient,
        cwd: &Path,
        script: &str,
        max_file: u64,
    ) -> Result<()> {
        batch(recipient, cwd, script, max_file).await
    }
}
pub(super) async fn send(root: &Path, operation: &Operation) -> Result<()> {
    send_with(root, operation, &SystemSftp).await
}
async fn send_with(root: &Path, operation: &Operation, sftp: &impl Sftp) -> Result<()> {
    operation.recipient.validate()?;
    let dir = operation_dir(root, &operation.id)?;
    let snapshot = dir.join("snapshot");
    let remote = format!("{}/{}", operation.recipient.directory, operation.id);
    let mut script = format!("mkdir {remote}\nchmod 700 {remote}\n");
    let manifest_bytes = serde_json::to_vec_pretty(operation)?;
    let (path, bytes) = (snapshot.join("manifest.json"), manifest_bytes.clone());
    blocking(move || {
        let mut manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        manifest.write_all(&bytes)?;
        Ok(manifest.sync_all()?)
    })
    .await?;
    for index in 0..operation.files.len() {
        script.push_str(&format!(
            "put {index}.log {remote}/{index}.log\nchmod 600 {remote}/{index}.log\n"
        ));
    }
    script.push_str(&format!(
        "put manifest.json {remote}/manifest.json\nchmod 600 {remote}/manifest.json\n"
    ));
    let run = async {
        sftp.run(&operation.recipient, &snapshot, &script, MAX_BYTES)
            .await?;
        let verify = tempfile::Builder::new()
            .prefix("verify-")
            .tempdir_in(&dir)?;
        std::fs::set_permissions(verify.path(), std::fs::Permissions::from_mode(0o700))?;
        let mut expected: Vec<_> = operation
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (format!("{i}.log"), f.bytes, f.sha256.clone()))
            .collect();
        expected.push((
            "manifest.json".into(),
            manifest_bytes.len() as u64,
            Some(format!("{:x}", Sha256::digest(&manifest_bytes))),
        ));
        for (name, bytes, sha256) in expected {
            let path = verify.path().join("copy.log");
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            sftp.run(
                &operation.recipient,
                verify.path(),
                &format!("get {remote}/{name} copy.log\n"),
                bytes,
            )
            .await?;
            blocking(move || {
                let mut copy = File::open(&path)?;
                if copy.metadata()?.len() != bytes {
                    return Err("Remote integrity check failed".into());
                }
                let mut hash = Sha256::new();
                io::copy(&mut copy, &mut hash)?;
                if Some(format!("{:x}", hash.finalize())) != sha256 {
                    return Err("Remote integrity check failed".into());
                }
                Ok(std::fs::remove_file(path)?)
            })
            .await?;
        }
        Ok(())
    };
    tokio::time::timeout(DEADLINE, run)
        .await
        .map_err(|_| "Transfer deadline exceeded; remote outcome uncertain")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_adapter_disables_config_hooks_and_brackets_ipv6() {
        let recipient = Recipient {
            user: "evidence".into(),
            host: "::1".into(),
            directory: "logs/phone".into(),
        };
        let cmd = command(&recipient, Path::new("/tmp"), 12);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(cmd.as_std().get_program(), "/usr/bin/sftp");
        assert_eq!(args.last(), Some(&"evidence@[::1]"));
        for required in [
            "/dev/null",
            "-oStrictHostKeyChecking=yes",
            "-oProxyCommand=none",
            "-oPermitLocalCommand=no",
            "-oControlPath=none",
        ] {
            assert!(args.contains(&required));
        }
        assert!(cmd
            .as_std()
            .get_envs()
            .all(|(key, _)| key == "SSH_AUTH_SOCK"));
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake {
        remote: RefCell<std::collections::HashMap<String, Vec<u8>>>,
        corrupt: bool,
        fail: bool,
    }
    impl Sftp for Fake {
        async fn run(&self, _: &Recipient, cwd: &Path, script: &str, max_file: u64) -> Result<()> {
            if self.fail {
                return Err("fixture transport refused".into());
            }
            for line in script.lines() {
                let args: Vec<_> = line.split_whitespace().collect();
                let mut remote = self.remote.borrow_mut();
                match args[0] {
                    "mkdir" => {
                        if remote.contains_key(args[1]) {
                            return Err("exists".into());
                        }
                        remote.insert(args[1].into(), vec![]);
                    }
                    "chmod" => {
                        assert!(matches!(args[1], "600" | "700"));
                        assert!(remote.contains_key(args[2]));
                    }
                    "put" => {
                        remote.insert(args[2].into(), std::fs::read(cwd.join(args[1]))?);
                    }
                    "get" => {
                        let mut bytes = remote.get(args[1]).ok_or("missing")?.clone();
                        if self.corrupt && !bytes.is_empty() {
                            bytes[0] ^= 1;
                        }
                        assert!(bytes.len() as u64 <= max_file);
                        std::fs::write(cwd.join(args[2]), bytes)?;
                    }
                    _ => panic!("not a closed SFTP verb"),
                }
            }
            Ok(())
        }
    }
    #[tokio::test]
    async fn roundtrip_checks_every_file_and_manifest_and_never_overwrites() {
        for (corrupt, fail) in [(false, false), (true, false), (false, true)] {
            let root = super::super::tests::fixture();
            let prepared = prepare(
                root.path(),
                super::super::tests::caller(),
                super::super::tests::recipient(),
            )
            .unwrap();
            let captured = snapshot(root.path(), &prepared.id, &prepared.caller).unwrap();
            let fake = Fake {
                remote: RefCell::default(),
                corrupt,
                fail,
            };
            assert_eq!(
                send_with(root.path(), &captured, &fake).await.is_ok(),
                !corrupt && !fail
            );
            if !corrupt && !fail {
                let before = fake.remote.borrow().clone();
                std::fs::remove_file(
                    operation_dir(root.path(), &captured.id)
                        .unwrap()
                        .join("snapshot/manifest.json"),
                )
                .unwrap();
                assert!(send_with(root.path(), &captured, &fake).await.is_err());
                assert_eq!(*fake.remote.borrow(), before);
            }
        }
    }
    #[tokio::test]
    async fn uncertain_timeout_does_not_retry() {
        struct Hung;
        impl Sftp for Hung {
            async fn run(&self, _: &Recipient, _: &Path, _: &str, _: u64) -> Result<()> {
                std::future::pending().await
            }
        }
        let root = super::super::tests::fixture();
        let op = prepare(
            root.path(),
            super::super::tests::caller(),
            super::super::tests::recipient(),
        )
        .unwrap();
        let op = snapshot(root.path(), &op.id, &op.caller).unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            send_with(root.path(), &op, &Hung)
        )
        .await
        .is_err());
    }
}

//! Fresh storage contract for the explicit disposable packaged-service role.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LIFETIME: std::time::Duration = std::time::Duration::from_secs(900);
pub const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct Scope {
    root: PathBuf,
}

impl Scope {
    pub fn create(root: &Path) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(io::Error::other("disposable root must be absolute"));
        }
        crate::create_new_storage_root(root)?;
        let scope = Self {
            root: root.canonicalize()?,
        };
        for dir in [scope.home(), scope.app_data(), scope.private_dir()] {
            crate::create_new_storage_root(&dir)?;
        }
        std::fs::write(scope.root.join("scope"), "disposable-v1\n")?;
        Ok(scope)
    }

    pub fn open(root: &Path) -> io::Result<Self> {
        let scope = Self {
            root: root.canonicalize()?,
        };
        if std::fs::read_to_string(scope.root.join("scope"))? != "disposable-v1\n" {
            return Err(io::Error::other("invalid disposable scope"));
        }
        scope.validate_storage()?;
        Ok(scope)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    pub fn app_data(&self) -> PathBuf {
        self.root.join("gateway")
    }
    pub fn private_dir(&self) -> PathBuf {
        self.root.join("private")
    }

    fn validate_storage(&self) -> io::Result<()> {
        for path in [self.home(), self.app_data(), self.private_dir()] {
            if path.canonicalize()? != path || !path.is_dir() {
                return Err(io::Error::other(
                    "disposable storage must not contain symlinks",
                ));
            }
        }
        Ok(())
    }

    fn require_fresh_storage(&self) -> io::Result<()> {
        self.validate_storage()?;
        for path in [self.home(), self.app_data(), self.private_dir()] {
            if std::fs::read_dir(path)?.next().is_some() {
                return Err(io::Error::other("disposable storage must be fresh"));
            }
        }
        Ok(())
    }

    pub fn claim_supervisor(&self) -> io::Result<()> {
        use std::io::Write;
        self.require_fresh_storage()?;
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join("supervisor.pid"))?;
        write!(marker, "{}", std::process::id())
    }

    pub fn supervisor_pid(&self) -> io::Result<u32> {
        std::fs::read_to_string(self.root.join("supervisor.pid"))?
            .parse()
            .map_err(io::Error::other)
    }

    pub fn claim_gateway(&self) -> io::Result<()> {
        self.require_fresh_storage()?;
        self.supervisor_pid()?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join("gateway-claimed"))?;
        Ok(())
    }

    pub fn validate_workspace(&self, path: &Path) -> io::Result<()> {
        let workspaces = self.app_data().join("workspaces");
        if !path.starts_with(&workspaces)
            || path == workspaces
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(io::Error::other("workspace escapes disposable storage"));
        }
        let mut ancestor = path;
        loop {
            match std::fs::symlink_metadata(ancestor) {
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| io::Error::other("workspace has no parent"))?;
                }
                Err(e) => return Err(e),
            }
        }
        if ancestor.canonicalize()? != ancestor {
            return Err(io::Error::other("disposable workspace contains a symlink"));
        }
        Ok(())
    }

    /// Start with no inherited credentials, service controls or topology.
    pub fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(self.home())
            .env("HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", self.root())
            // A signed .app normally uses the host Keychain. Disposable keys
            // must stay under this scope's private directory, including backups.
            .env("LUCIDOS_SECRET_KEY_STORE", "file")
            .env("LUCIDOS_GATEWAY_DATA", self.app_data())
            .env("LUCIDOS_PRIVATE_DIR", self.private_dir());
        // macOS cannot nest seatbelts. Keep the existing inherited-sandbox
        // fact through every env_clear so engines select their inherited mode.
        if std::env::var("LUCIDOS_AGENT_SANDBOXED").as_deref() == Ok("1") {
            command.env("LUCIDOS_AGENT_SANDBOXED", "1");
        }
        command
    }
}

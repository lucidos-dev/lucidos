# Upstream issue: macOS `install_inner` deletes the backup instead of restoring it

**Status: not filed.** This is a ready-to-paste issue body for
`tauri-apps/plugins-workspace`. It is kept here because Lucidos ships the
affected code and detects the outcome locally
(`crates/lucidos-app/src/updater.rs`, `installed_bundle_fault`, asked on BOTH the
`Ok` and the `Err` branch of the install, since the destructive case below lands
on the `Err` one). That check is NOT a temporary measure and carries no removal
condition: it answers "is there something runnable to restart into", which our
`KeepAlive` launchd job makes load-bearing whatever upstream does. A fix here
would make it fire less often, not make it redundant. Everything below the rule
is the issue text.

Related local material: finding F9 in
`docs/audits/2026-08-02-macos-update-path-audit.md`.

---

## `tauri-plugin-updater` (macOS): a failed bundle swap deletes the backup instead of restoring it

### Version

`tauri-plugin-updater` 2.10.1, `src/updater.rs`, the macOS `install_inner`
(lines 1245 to 1307 in the published crate).

### What happens

On macOS the installer moves the currently installed `.app` into a `tempfile::TempDir`
before putting the new bundle in place:

```rust
let tmp_backup_dir = tempfile::Builder::new()
    .prefix("tauri_current_app")
    .tempdir()?;

// ...

// Try to move the current app to backup
let move_result = std::fs::rename(
    &self.extract_path,
    tmp_backup_dir.path().join("current_app"),
);
```

The final step then removes anything still at `extract_path` and renames the
freshly extracted tree onto it:

```rust
} else {
    // Remove existing directory if it exists
    if self.extract_path.exists() {
        std::fs::remove_dir_all(&self.extract_path)?;
    }
    // Move the new app to the target path
    std::fs::rename(tmp_extract_dir.path(), &self.extract_path)?;
}
```

Neither of those two `?`s has a restore branch. When either fails, the function
returns `Err`, `tmp_backup_dir` is dropped at the end of the scope, and
`TempDir`'s `Drop` deletes the directory tree, taking `current_app` with it. The
user is left with **no application at `extract_path` at all**: not the new
version, and not the one they had before.

The variable is named `tmp_backup_dir` and the comment above the rename says
"Try to move the current app to backup", so the code reads as though a rollback
exists. It does not.

The AppleScript branch a few lines up (the `need_authorization` path, taken when
the first rename fails with `PermissionDenied`) has the same shape: its
`rm -rf '<src>' && mv -f '<new>' '<src>'` can fail after the `rm`, and the error
path only removes the extract dir.

### How to reproduce

Anything that makes the final `rename` fail after the backup move succeeded. The
easiest deterministic one is to make the parent directory read-only between the
two operations. In the wild the plausible causes are a full disk, an endpoint
security or antivirus agent holding a handle on the directory, a `.app` whose
parent is on a different device than the temp dir so `rename` returns `EXDEV`,
or the user's machine going down mid-install.

### Why it matters more than a failed update

A failed update that leaves the old version in place is a retry. A failed update
that leaves nothing in place is an app the user cannot start, from an app they
can no longer start, so the in-app updater is not a route back. The recovery is
a manual re-download.

It gets worse for an app whose background service points into the bundle. In our
case a launchd agent with `KeepAlive=true` runs
`<bundle>/Contents/MacOS/<binary>`; with the bundle gone that job crash-loops on
its `ThrottleInterval` forever. The currently running process keeps the deleted
inode alive, so nothing looks wrong until the next restart or reboot, at which
point the whole stack is down with no obvious connection to an update that
happened days earlier.

### Suggested fix

Give the failure paths the restore branch the backup was taken for. Sketch:

```rust
} else {
    if self.extract_path.exists() {
        std::fs::remove_dir_all(&self.extract_path)?;
    }
    if let Err(err) = std::fs::rename(tmp_extract_dir.path(), &self.extract_path) {
        // Put the user's app back before giving up. A failed restore is worth
        // reporting separately: at that point the backup is the only copy and
        // its location is the one useful thing left to tell them.
        let backup = tmp_backup_dir.path().join("current_app");
        if backup.exists() {
            std::fs::rename(&backup, &self.extract_path).map_err(|restore_err| {
                Error::Io(std::io::Error::other(format!(
                    "failed to install the update ({err}) and failed to restore the \
                     previous app from {} ({restore_err})",
                    backup.display()
                )))
            })?;
        }
        return Err(err.into());
    }
}
```

The same treatment applies to the `remove_dir_all` above it and to the
AppleScript branch's failure path.

Two smaller things worth doing alongside it:

- **Do not let `TempDir`'s `Drop` be what deletes the only copy of the user's
  app.** Persisting the backup dir (`TempDir::into_path`) on the error path, and
  naming it in the returned error, means a restore that itself fails still leaves
  the user something to drag back manually.
- **Consider verifying before declaring success.** A cheap check that the main
  executable exists at `extract_path` after the swap would turn a whole class of
  partial-unpack failures into a reported error rather than a silent one. We
  ended up adding exactly that check in our own app because we could not add it
  here.

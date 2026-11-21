use super::{
    handle::Handle,
    manifest::{Manifest, Verification},
    store::{regular_file, Store},
    BoxError,
};
use crate::core::{backup::capture::FileIndexEntry, secret_store::SecretKey};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

pub fn component_hash(path: &Path) -> Result<(u64, String), BoxError> {
    regular_file(path)?;
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        bytes = bytes
            .checked_add(read as u64)
            .ok_or("snapshot size overflow")?;
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}

pub fn verify_components(directory: &Path, manifest: &Manifest) -> Result<(), BoxError> {
    for component in &manifest.components {
        let (bytes, sha256) = component_hash(&directory.join(component.name.as_str()))?;
        if bytes != component.bytes || sha256 != component.sha256 {
            return Err("snapshot component hash mismatch".into());
        }
    }
    verify_index(directory, manifest)
}

/// Read both streams together. Every archive entry must have exactly one index
/// record, in sorted order, with the same path, mode, length and content hash.
pub fn verify_index(directory: &Path, manifest: &Manifest) -> Result<(), BoxError> {
    let mut index = BufReader::new(zstd::Decoder::new(File::open(
        directory.join("files.index.jsonl.zst"),
    )?)?);
    let mut archive = tar::Archive::new(zstd::Decoder::new(File::open(
        directory.join("files.tar.zst"),
    )?)?);
    let mut tree = Sha256::new();
    let mut count = 0u64;
    let mut bytes = 0u64;
    let mut previous = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let mut line = Vec::new();
        if index.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            return Err("snapshot index is truncated".into());
        }
        tree.update(&line);
        let record: FileIndexEntry = serde_json::from_slice(&line)?;
        let path = entry.path()?.into_owned();
        if !super::restore_files::restorable_path(&path)
            || previous.as_ref().is_some_and(|old| old >= &path)
            || !entry.header().entry_type().is_file()
            || path != record.path
            || entry.size() != record.size
            || entry.header().mode()? != record.mode
        {
            return Err("snapshot file index mismatch".into());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let len = entry.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            hash.update(&buffer[..len]);
        }
        if format!("{:x}", hash.finalize()) != record.sha256 {
            return Err("snapshot file content mismatch".into());
        }
        previous = Some(path);
        count = count.checked_add(1).ok_or("snapshot count overflow")?;
        bytes = bytes
            .checked_add(record.size)
            .ok_or("snapshot size overflow")?;
    }
    let mut extra = [0u8; 1];
    if index.read(&mut extra)? != 0
        || count != manifest.files.count
        || bytes != manifest.files.bytes
        || format!("{:x}", tree.finalize()) != manifest.files.tree_sha256
    {
        return Err("snapshot file summary mismatch".into());
    }
    Ok(())
}

pub fn check(
    store: &Store,
    handle: &Handle,
    key: &SecretKey,
    database_url: &str,
) -> Result<Manifest, BoxError> {
    let mut manifest = store.get(handle, key)?;
    let directory = store.snapshot_dir(handle);
    verify_components(&directory, &manifest)?;
    verify_dump(&directory.join("db.dump"), database_url)?;
    manifest.verified = Some(Verification {
        mac: true,
        at: chrono::Utc::now(),
    });
    Ok(manifest)
}

pub fn verify_dump(path: &Path, database_url: &str) -> Result<(), BoxError> {
    // --list parses only the local dump; it never connects to a database.
    let output = crate::core::backup::pg_tool_command("pg_restore", database_url)?
        .arg("--list")
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err("snapshot database dump is invalid".into());
    }
    Ok(())
}

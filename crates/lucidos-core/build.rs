use std::{fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    check_migration_versions(Path::new("migrations"));
}

fn check_migration_versions(migrations_dir: &Path) {
    let entries = match fs::read_dir(migrations_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut by_version: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        let version = match name.split_once('_') {
            Some((v, _)) => v.to_string(),
            None => continue,
        };
        by_version.entry(version).or_default().push(name);
    }
    let dupes: Vec<_> = by_version.iter().filter(|(_, v)| v.len() > 1).collect();
    if !dupes.is_empty() {
        let mut msg = String::from("Duplicate migration version prefix(es) detected:\n");
        for (version, files) in dupes {
            msg.push_str(&format!("  {}: {}\n", version, files.join(", ")));
        }
        msg.push_str(
            "Rename one to a unique YYYYMMDDHHMMSS timestamp (use scripts/new-migration.sh).",
        );
        panic!("{}", msg);
    }
}

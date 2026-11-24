//! Writes `thread-event-wire.ts` at its repository path below `--out <dir>`,
//! reading the Rust source under `--repo-root <dir>`. It compiles no engine
//! crate.

use std::path::PathBuf;

const USAGE: &str = "usage: lucidos-wire-codegen --out <dir> --repo-root <dir>";

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let (mut out, mut repo_root) = (None, None);
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--out" => &mut out,
            "--repo-root" => &mut repo_root,
            other => return Err(format!("unknown argument {other}; {USAGE}").into()),
        };
        match args.next() {
            Some(dir) if !dir.starts_with("--") => *slot = Some(PathBuf::from(dir)),
            _ => return Err(format!("{arg} needs a directory; {USAGE}").into()),
        }
    }
    let (Some(out), Some(repo_root)) = (out, repo_root) else {
        return Err(USAGE.into());
    };
    let path = out.join(lucidos_wire_codegen::WIRE_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, lucidos_wire_codegen::render(&repo_root))?;
    Ok(())
}

//! Writes every generated file under `--out <dir>`, reading the engine source
//! under `--repo-root <dir>`. `scripts/codegen.sh` runs it, and nothing ships
//! it: every `cargo build -p lucidos-engine` builds it beside the engine for
//! free, since it shares the engine's lib.

use std::path::PathBuf;
use std::process::ExitCode;

use lucidos_engine::codegen::{render_all, write_all};
use lucidos_engine::log;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut out, mut repo_root) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().map(PathBuf::from),
            "--repo-root" => repo_root = args.next().map(PathBuf::from),
            other => {
                log!("[Codegen] unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(out), Some(repo_root)) = (out, repo_root) else {
        log!("[Codegen] usage: lucidos-codegen --out <dir> --repo-root <dir>");
        return ExitCode::from(2);
    };
    match write_all(&render_all(&repo_root), &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log!("[Codegen] cannot write under {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

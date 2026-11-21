//! Renders the manifest-generated subcommands into `OUT_DIR`, from the
//! `lucidos-capabilities` leaf crate. `main.rs` includes them as `mod generated`.

fn main() {
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script");
    let path = std::path::Path::new(&out).join("cli_commands.rs");
    std::fs::write(&path, lucidos_capabilities::codegen::render_cli_module())
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

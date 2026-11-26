fn main() {
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script");
    let path = std::path::Path::new(&out).join("cli_commands.rs");
    std::fs::write(&path, lucidos_capabilities::codegen::render_cli_module())
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

use std::path::Path;

/// INVARIANT: Writes an executable stub through a child `cp`, so this process never
/// holds it open for writing. Linux refuses to exec a file that a parallel
/// test's fork still holds open that way ("Text file busy").
pub(crate) fn write_stub_executable(script: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let mut source_name = script.file_name().expect("stub file name").to_os_string();
    source_name.push(".src");
    let source = script.with_file_name(source_name);
    std::fs::write(&source, body).expect("write stub source");
    let copied = std::process::Command::new("/bin/cp")
        .arg(&source)
        .arg(script)
        .status()
        .expect("run cp");
    assert!(copied.success(), "cp the stub into place");
    // INVARIANT: The copy's mode follows the umask, and a chmod opens nothing for writing.
    std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

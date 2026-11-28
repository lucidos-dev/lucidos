use lucidos_engine::core::snapshot::{capacity::preflight, store::install_roomy_volumes_for_test};

#[test]
fn an_installed_fixture_makes_every_volume_roomy_not_only_the_workspace_home() {
    install_roomy_volumes_for_test();
    let outside_home = tempfile::tempdir().unwrap();
    const GIB: u64 = 1 << 30;
    preflight(outside_home.path(), outside_home.path(), 400 * GIB, 0)
        .expect("a database volume outside the fixture home must read as roomy");
}

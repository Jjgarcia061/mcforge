//! Regenerates the UI bindings. CI runs this and fails if the committed file is stale.

#[test]
fn export_typescript_bindings() {
    mcforge_lib::export_bindings(&mcforge_lib::specta_builder());
    assert!(std::path::Path::new(mcforge_lib::BINDINGS_PATH).exists());
}

#[test]
fn derive_nano_map_expands_for_named_structs() {
    let test = trybuild::TestCases::new();
    test.pass("tests/ui/nanomap_pass.rs");
}

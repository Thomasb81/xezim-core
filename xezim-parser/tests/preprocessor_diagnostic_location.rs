use std::path::Path;

use sv_parser::preprocessor::Preprocessor;

#[test]
fn reserved_macro_redefinition_reports_source_path_and_line() {
    let mut pp = Preprocessor::new();
    pp.preprocess_file(
        "\n`define __FILE__ fake\n`define __LINE__ 9\n",
        Some(Path::new("/work/dram_ut/top.sv")),
    );

    assert_eq!(pp.errors().len(), 2);
    assert!(
        pp.errors()[0].starts_with("/work/dram_ut/top.sv:2: `__FILE__"),
        "unexpected diagnostic: {}",
        pp.errors()[0]
    );
    assert!(
        pp.errors()[1].starts_with("/work/dram_ut/top.sv:3: `__LINE__"),
        "unexpected diagnostic: {}",
        pp.errors()[1]
    );
}

#[test]
fn reserved_macro_redefinition_in_include_reports_include_location() {
    let unique = format!(
        "xezim-pp-location-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).expect("create temp include directory");
    let include = dir.join("norand_defs.svh");
    std::fs::write(
        &include,
        "// compatibility defines\n`define __FILE__ disabled\n",
    )
    .expect("write include");

    let top = dir.join("top.sv");
    let mut pp = Preprocessor::new();
    pp.preprocess_file("`include \"norand_defs.svh\"\n", Some(&top));

    let expected = format!("{}:2: `__FILE__", include.display());
    assert_eq!(pp.errors().len(), 1);
    assert!(
        pp.errors()[0].starts_with(&expected),
        "expected prefix {expected:?}, got {:?}",
        pp.errors()[0]
    );

    std::fs::remove_dir_all(dir).expect("remove temp include directory");
}

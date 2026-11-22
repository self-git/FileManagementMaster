use std::{env, path::PathBuf};

#[test]
fn bundled_git_references_recovery_and_gc() {
    let directory = env::var_os("FILEMANAGEMENT_BUNDLED_GIT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/git"));
    let report = filemanagement_master::verify_runtime(&directory)
        .unwrap_or_else(|error| panic!("随包 Git 验收失败 {error:#}"));
    assert_eq!(report.ref_format, "reftable");
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

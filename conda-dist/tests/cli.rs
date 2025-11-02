use trycmd::TestCases;

#[test]
fn lock_flag_behaviour() {
    let cases = TestCases::new();
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_conda_dist") {
        let _ = cases
            .register_bin("conda-dist", std::path::PathBuf::from(bin))
            .default_bin_name("conda-dist");
    }
    cases.case("tests/cases/lock-missing.toml");
    cases.case("tests/cases/lock-stale.toml");
    cases.case("tests/cases/lock-conflict.toml");
    cases.case("tests/cases/lock-fresh.toml");
}

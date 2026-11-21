use core_test_support::test_codex_exec::test_codex_exec;
use predicates::str::contains;

#[test]
fn exec_rejects_explicit_sandbox_settings() -> anyhow::Result<()> {
    for config in [
        "[windows]\nsandbox = 'elevated'",
        "[sandbox_workspace_write]\nwritable_roots = []",
    ] {
        let test = test_codex_exec();
        std::fs::write(test.home_path().join("config.toml"), config)?;
        test.cmd()
            .args(["--skip-git-repo-check", "hello"])
            .assert()
            .failure()
            .stderr(contains("sandbox configuration is unsupported"));
    }
    Ok(())
}

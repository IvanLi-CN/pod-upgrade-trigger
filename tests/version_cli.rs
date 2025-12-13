use std::process::Command;

fn expected_tag() -> String {
    if let Some(tag) = option_env!("PODUP_BUILD_TAG") {
        let trimmed = tag.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let version = option_env!("PODUP_BUILD_VERSION")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"));

    format!("v{version}")
}

#[test]
fn version_flag_outputs_current_release_tag() {
    let exe = env!("CARGO_BIN_EXE_pod-upgrade-trigger");
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("failed to run pod-upgrade-trigger --version");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, expected_tag());
}

#[test]
fn version_subcommand_outputs_current_release_tag() {
    let exe = env!("CARGO_BIN_EXE_pod-upgrade-trigger");
    let output = Command::new(exe)
        .arg("version")
        .output()
        .expect("failed to run pod-upgrade-trigger version");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, expected_tag());
}

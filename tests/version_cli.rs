use std::process::Command;

fn expected_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
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


use std::path::Path;

#[test]
fn no_systemd_socket_unit_present() {
    assert!(
        !Path::new("systemd/pod-upgrade-trigger.socket").exists(),
        "systemd/pod-upgrade-trigger.socket should not exist; socket activation is no longer supported"
    );
}

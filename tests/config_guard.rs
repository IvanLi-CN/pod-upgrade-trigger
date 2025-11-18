use std::path::Path;

#[test]
fn no_systemd_socket_unit_present() {
    assert!(
        !Path::new("systemd/webhook-auto-update.socket").exists(),
        "systemd/webhook-auto-update.socket should not exist; socket activation is no longer supported"
    );
}


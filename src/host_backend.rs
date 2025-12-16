use std::path::{Component, Path};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostBackendKind {
    Local,
    Ssh,
}

impl HostBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostBackendConfig {
    pub ssh_target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemdUnitName(String);

impl SystemdUnitName {
    pub fn parse(raw: &str) -> Result<Self, String> {
        validate_systemd_unit_name(raw)?;
        Ok(Self(raw.trim().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostAbsPath(String);

impl HostAbsPath {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("path-empty".to_string());
        }
        let path = Path::new(trimmed);
        validate_host_abs_path(path)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct HostFileMeta {
    pub is_file: bool,
    pub is_dir: bool,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub enum HostBackendError {
    InvalidInput(String),
    ExecFailed(String),
    NonZeroExit { exit: Option<i32>, stderr: String },
    Io(String),
}

impl HostBackendError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid-input",
            Self::ExecFailed(_) => "exec-failed",
            Self::NonZeroExit { .. } => "non-zero-exit",
            Self::Io(_) => "io",
        }
    }
}

pub trait HostBackend: Send + Sync {
    fn kind(&self) -> HostBackendKind;

    fn ssh_target_hint(&self) -> Option<String> {
        None
    }

    fn podman(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError>;
    fn systemctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError>;
    fn journalctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError>;
    fn busctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError>;

    fn exists(&self, path: &HostAbsPath) -> Result<bool, HostBackendError>;
    fn is_dir(&self, path: &HostAbsPath) -> Result<bool, HostBackendError>;
    fn is_file(&self, path: &HostAbsPath) -> Result<bool, HostBackendError>;

    fn list_dir(&self, path: &HostAbsPath) -> Result<Vec<String>, HostBackendError>;
    fn read_file_to_string(&self, path: &HostAbsPath) -> Result<String, HostBackendError>;
    fn metadata(&self, path: &HostAbsPath) -> Result<HostFileMeta, HostBackendError>;
}

#[derive(Clone, Debug)]
pub struct LocalHostBackend;

impl LocalHostBackend {
    pub fn new() -> Self {
        Self
    }
}

impl HostBackend for LocalHostBackend {
    fn kind(&self) -> HostBackendKind {
        HostBackendKind::Local
    }

    fn podman(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        exec_local("podman", args).map_err(HostBackendError::ExecFailed)
    }

    fn systemctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push("--user".to_string());
        full.extend(args.iter().cloned());
        exec_local("systemctl", &full).map_err(HostBackendError::ExecFailed)
    }

    fn journalctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push("--user".to_string());
        full.extend(args.iter().cloned());
        exec_local("journalctl", &full).map_err(HostBackendError::ExecFailed)
    }

    fn busctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push("--user".to_string());
        full.extend(args.iter().cloned());
        exec_local("busctl", &full).map_err(HostBackendError::ExecFailed)
    }

    fn exists(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        match std::fs::metadata(path.as_path()) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(HostBackendError::Io(err.to_string())),
        }
    }

    fn is_dir(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        match std::fs::metadata(path.as_path()) {
            Ok(meta) => Ok(meta.is_dir()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(HostBackendError::Io(err.to_string())),
        }
    }

    fn is_file(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        match std::fs::metadata(path.as_path()) {
            Ok(meta) => Ok(meta.is_file()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(HostBackendError::Io(err.to_string())),
        }
    }

    fn list_dir(&self, path: &HostAbsPath) -> Result<Vec<String>, HostBackendError> {
        let read_dir = std::fs::read_dir(path.as_path()).map_err(|e| HostBackendError::Io(e.to_string()))?;
        let mut out = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            out.push(name.to_string());
        }
        Ok(out)
    }

    fn read_file_to_string(&self, path: &HostAbsPath) -> Result<String, HostBackendError> {
        std::fs::read_to_string(path.as_path()).map_err(|e| HostBackendError::Io(e.to_string()))
    }

    fn metadata(&self, path: &HostAbsPath) -> Result<HostFileMeta, HostBackendError> {
        let meta = std::fs::metadata(path.as_path()).map_err(|e| HostBackendError::Io(e.to_string()))?;
        let modified = meta.modified().ok();
        Ok(HostFileMeta {
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            modified,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SshHostBackend {
    target: String,
    default_opts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FailingHostBackend {
    kind: HostBackendKind,
    ssh_hint: Option<String>,
    err: String,
}

impl FailingHostBackend {
    pub fn ssh(err: String, ssh_target_raw: Option<String>) -> Self {
        let ssh_hint = ssh_target_raw.as_deref().map(ssh_target_hint);
        Self {
            kind: HostBackendKind::Ssh,
            ssh_hint,
            err,
        }
    }
}

impl HostBackend for FailingHostBackend {
    fn kind(&self) -> HostBackendKind {
        self.kind
    }

    fn ssh_target_hint(&self) -> Option<String> {
        self.ssh_hint.clone()
    }

    fn podman(&self, _args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn systemctl_user(&self, _args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn journalctl_user(&self, _args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn busctl_user(&self, _args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn exists(&self, _path: &HostAbsPath) -> Result<bool, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn is_dir(&self, _path: &HostAbsPath) -> Result<bool, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn is_file(&self, _path: &HostAbsPath) -> Result<bool, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn list_dir(&self, _path: &HostAbsPath) -> Result<Vec<String>, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn read_file_to_string(&self, _path: &HostAbsPath) -> Result<String, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }

    fn metadata(&self, _path: &HostAbsPath) -> Result<HostFileMeta, HostBackendError> {
        Err(HostBackendError::ExecFailed(self.err.clone()))
    }
}

impl SshHostBackend {
    pub fn new(target: String) -> Result<Self, String> {
        validate_ssh_target(&target)?;
        Ok(Self {
            target,
            default_opts: vec![
                "-oBatchMode=yes".to_string(),
                "-oStrictHostKeyChecking=accept-new".to_string(),
                "-oConnectTimeout=5".to_string(),
                "-oConnectionAttempts=1".to_string(),
            ],
        })
    }

    pub fn ssh_argv_for_test(&self, remote_argv: &[String]) -> Result<Vec<String>, HostBackendError> {
        validate_remote_argv(remote_argv)?;
        let mut argv = Vec::new();
        argv.extend(self.default_opts.iter().cloned());
        argv.push(self.target.clone());
        argv.extend(remote_argv.iter().cloned());
        Ok(argv)
    }

    fn exec_remote(&self, remote_argv: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        validate_remote_argv(remote_argv)?;

        let mut cmd = Command::new("ssh");
        for opt in &self.default_opts {
            cmd.arg(opt);
        }
        cmd.arg(&self.target);
        for part in remote_argv {
            cmd.arg(part);
        }

        let mut result = crate::run_quiet_command(cmd)
            .map_err(|e| HostBackendError::ExecFailed(redact_ssh_error(&self.target, &e)))?;

        // Avoid leaking full targets (IPs/usernames) into logs and task meta
        // when the target is not a simple ssh config alias.
        if ssh_target_hint(&self.target) == "<redacted>" {
            result.stdout = result.stdout.replace(&self.target, "<redacted>");
            result.stderr = result.stderr.replace(&self.target, "<redacted>");
        }

        Ok(result)
    }

    fn exists_via_test(&self, flag: &str, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        let remote = vec![
            "test".to_string(),
            flag.to_string(),
            path.as_str().to_string(),
        ];
        let result = self.exec_remote(&remote)?;
        if result.success() {
            return Ok(true);
        }
        match result.status.code() {
            Some(1) => Ok(false),
            other => Err(HostBackendError::NonZeroExit {
                exit: other,
                stderr: result.stderr,
            }),
        }
    }
}

impl HostBackend for SshHostBackend {
    fn kind(&self) -> HostBackendKind {
        HostBackendKind::Ssh
    }

    fn ssh_target_hint(&self) -> Option<String> {
        Some(ssh_target_hint(&self.target))
    }

    fn podman(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut remote = Vec::with_capacity(args.len() + 1);
        remote.push("podman".to_string());
        remote.extend(args.iter().cloned());
        self.exec_remote(&remote)
    }

    fn systemctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut remote = Vec::with_capacity(args.len() + 2);
        remote.push("systemctl".to_string());
        remote.push("--user".to_string());
        remote.extend(args.iter().cloned());
        self.exec_remote(&remote)
    }

    fn journalctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut remote = Vec::with_capacity(args.len() + 2);
        remote.push("journalctl".to_string());
        remote.push("--user".to_string());
        remote.extend(args.iter().cloned());
        self.exec_remote(&remote)
    }

    fn busctl_user(&self, args: &[String]) -> Result<crate::CommandExecResult, HostBackendError> {
        let mut remote = Vec::with_capacity(args.len() + 2);
        remote.push("busctl".to_string());
        remote.push("--user".to_string());
        remote.extend(args.iter().cloned());

        let result = self.exec_remote(&remote)?;
        if result.success() {
            return Ok(result);
        }

        // Preserve the existing "fallback when busctl is not available at all"
        // behaviour by treating the common "command not found" exit code as
        // an execution failure instead of a normal non-zero exit.
        if result.status.code() == Some(127) {
            return Err(HostBackendError::ExecFailed("busctl-not-found".to_string()));
        }

        Ok(result)
    }

    fn exists(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        self.exists_via_test("-e", path)
    }

    fn is_dir(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        self.exists_via_test("-d", path)
    }

    fn is_file(&self, path: &HostAbsPath) -> Result<bool, HostBackendError> {
        self.exists_via_test("-f", path)
    }

    fn list_dir(&self, path: &HostAbsPath) -> Result<Vec<String>, HostBackendError> {
        // NOTE: We must avoid shell quoting here. `ssh host ls -1A -- /abs/path`
        // becomes a single remote command string; inputs are constrained by
        // validate_host_abs_path + validate_shell_token.
        let remote = vec![
            "ls".to_string(),
            "-1A".to_string(),
            "--".to_string(),
            path.as_str().to_string(),
        ];
        let result = self.exec_remote(&remote)?;
        if !result.success() {
            return Err(HostBackendError::NonZeroExit {
                exit: result.status.code(),
                stderr: result.stderr,
            });
        }
        let mut out = Vec::new();
        for line in result.stdout.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Treat remote filenames as untrusted; only accept safe basenames.
            if validate_dir_entry_name(trimmed).is_ok() {
                out.push(trimmed.to_string());
            }
        }
        Ok(out)
    }

    fn read_file_to_string(&self, path: &HostAbsPath) -> Result<String, HostBackendError> {
        let remote = vec![
            "cat".to_string(),
            "--".to_string(),
            path.as_str().to_string(),
        ];
        let result = self.exec_remote(&remote)?;
        if !result.success() {
            return Err(HostBackendError::NonZeroExit {
                exit: result.status.code(),
                stderr: result.stderr,
            });
        }
        Ok(result.stdout)
    }

    fn metadata(&self, path: &HostAbsPath) -> Result<HostFileMeta, HostBackendError> {
        let is_dir = self.is_dir(path)?;
        let is_file = self.is_file(path)?;

        // Only attempt stat() for regular files.
        let modified = if is_file {
            let remote = vec![
                "stat".to_string(),
                "-c".to_string(),
                "%Y".to_string(),
                "--".to_string(),
                path.as_str().to_string(),
            ];
            let result = self.exec_remote(&remote)?;
            if !result.success() {
                None
            } else {
                result
                    .stdout
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
            }
        } else {
            None
        };

        Ok(HostFileMeta {
            is_file,
            is_dir,
            modified,
        })
    }
}

fn exec_local(program: &str, args: &[String]) -> Result<crate::CommandExecResult, String> {
    let mut cmd = Command::new(program);
    for arg in args {
        cmd.arg(arg);
    }
    crate::run_quiet_command(cmd)
}

pub fn validate_systemd_unit_name(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("unit-empty".to_string());
    }
    if !trimmed.ends_with(".service") {
        return Err("unit-not-service".to_string());
    }
    if trimmed.len() > 200 {
        return Err("unit-too-long".to_string());
    }
    if trimmed.contains('/') {
        return Err("unit-invalid-char-/".to_string());
    }
    for ch in trimmed.chars() {
        let ok = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '@');
        if !ok {
            return Err("unit-invalid-char".to_string());
        }
    }
    Ok(())
}

pub fn validate_host_abs_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("path-not-absolute".to_string());
    }
    let Some(raw) = path.to_str() else {
        return Err("path-not-utf8".to_string());
    };
    if raw.trim().is_empty() {
        return Err("path-empty".to_string());
    }
    if raw.len() > 4096 {
        return Err("path-too-long".to_string());
    }
    // Disallow obvious shell metacharacters to keep ssh remote command strings safe.
    if raw.chars().any(is_disallowed_shell_char) {
        return Err("path-unsafe-char".to_string());
    }
    for comp in path.components() {
        match comp {
            Component::RootDir => {}
            Component::Normal(seg) => {
                let Some(seg) = seg.to_str() else {
                    return Err("path-seg-not-utf8".to_string());
                };
                if seg.is_empty() {
                    return Err("path-seg-empty".to_string());
                }
                if seg == "." || seg == ".." {
                    return Err("path-dot-seg".to_string());
                }
                for ch in seg.chars() {
                    let ok = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' );
                    if !ok {
                        return Err("path-invalid-char".to_string());
                    }
                }
            }
            _ => return Err("path-invalid-component".to_string()),
        }
    }
    Ok(())
}

pub fn validate_dir_entry_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name-empty".to_string());
    }
    if name == "." || name == ".." {
        return Err("name-dot".to_string());
    }
    if name.contains('/') {
        return Err("name-has-slash".to_string());
    }
    if name.len() > 255 {
        return Err("name-too-long".to_string());
    }
    if name.chars().any(is_disallowed_shell_char) {
        return Err("name-unsafe-char".to_string());
    }
    Ok(())
}

pub fn validate_ssh_target(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("ssh-target-empty".to_string());
    }
    if trimmed.len() > 512 {
        return Err("ssh-target-too-long".to_string());
    }
    // Prevent option injection: `ssh <opts> <target> ...` treats a leading '-'
    // as another option instead of a destination.
    if trimmed.starts_with('-') {
        return Err("ssh-target-invalid-leading-dash".to_string());
    }
    if trimmed.chars().any(is_disallowed_shell_char) {
        return Err("ssh-target-unsafe-char".to_string());
    }
    Ok(())
}

fn validate_shell_token(token: &str) -> Result<(), HostBackendError> {
    if token.trim().is_empty() {
        return Err(HostBackendError::InvalidInput("token-empty".to_string()));
    }
    if token.chars().any(is_disallowed_shell_char) {
        return Err(HostBackendError::InvalidInput("token-unsafe-char".to_string()));
    }
    Ok(())
}

fn validate_remote_argv(remote_argv: &[String]) -> Result<(), HostBackendError> {
    if remote_argv.is_empty() {
        return Err(HostBackendError::InvalidInput("remote-argv-empty".to_string()));
    }
    // Whitelist the leading command token.
    match remote_argv[0].as_str() {
        "podman" | "systemctl" | "journalctl" | "busctl" | "ls" | "cat" | "test" | "stat" => {}
        _ => return Err(HostBackendError::InvalidInput("remote-command-not-allowed".to_string())),
    }
    for token in remote_argv {
        validate_shell_token(token)?;
    }
    Ok(())
}

fn is_disallowed_shell_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            ';' | '|' | '&' | '$' | '(' | ')' | '`' | '"' | '\'' | '<' | '>' | '\\'
        )
}

pub fn ssh_target_hint(target: &str) -> String {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    // If the target looks like a simple ssh config alias, it is typically safe
    // to show it verbatim in logs. Otherwise, redact (may include IPs/usernames).
    let looks_like_alias = trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'));
    if looks_like_alias {
        trimmed.to_string()
    } else {
        "<redacted>".to_string()
    }
}

fn redact_ssh_error(target: &str, err: &str) -> String {
    let hint = ssh_target_hint(target);
    if hint == "<redacted>" {
        err.replace(target, "<redacted>")
    } else {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_unit_name_allows_common_units() {
        assert!(validate_systemd_unit_name("podup-e2e-noop.service").is_ok());
        assert!(validate_systemd_unit_name("svc-alpha@1.service").is_ok());
        assert!(validate_systemd_unit_name("a_b-c.d.service").is_ok());
    }

    #[test]
    fn validate_unit_name_rejects_unsafe_units() {
        assert!(validate_systemd_unit_name("").is_err());
        assert!(validate_systemd_unit_name("not-a-service.timer").is_err());
        assert!(validate_systemd_unit_name("bad;rm -rf /.service").is_err());
        assert!(validate_systemd_unit_name("bad name.service").is_err());
        assert!(validate_systemd_unit_name("a/b.service").is_err());
    }

    #[test]
    fn validate_abs_path_basic() {
        assert!(HostAbsPath::parse("/home/ivan/.local/share/podman-auto-update/logs").is_ok());
        assert!(HostAbsPath::parse("relative/path").is_err());
        assert!(HostAbsPath::parse("/tmp/has space").is_err());
        assert!(HostAbsPath::parse("/tmp/evil;rm").is_err());
        assert!(HostAbsPath::parse("/tmp/..").is_err());
    }

    #[test]
    fn ssh_command_includes_required_options() {
        let backend = SshHostBackend::new("podup-test".to_string()).unwrap();
        let remote = vec!["podman".to_string(), "--version".to_string()];
        let argv = backend.ssh_argv_for_test(&remote).unwrap();

        assert!(argv.iter().any(|a| a == "-oBatchMode=yes"));
        assert!(argv.iter().any(|a| a == "-oStrictHostKeyChecking=accept-new"));
        assert!(argv.iter().any(|a| a == "-oConnectTimeout=5"));
        assert!(argv.iter().any(|a| a == "-oConnectionAttempts=1"));
        assert!(argv.iter().any(|a| a == "podup-test"));
        assert!(argv.iter().any(|a| a == "podman"));
    }

    #[test]
    fn validate_ssh_target_rejects_unsafe() {
        assert!(validate_ssh_target("podup-test").is_ok());
        assert!(validate_ssh_target("ivan@192.168.31.15").is_ok());
        assert!(validate_ssh_target("ivan@192.168.31.15:2222").is_ok());
        assert!(validate_ssh_target("ssh://ivan@192.168.31.15:2222").is_ok());
        assert!(validate_ssh_target("-oProxyCommand=sh").is_err());
        assert!(validate_ssh_target("bad target").is_err());
        assert!(validate_ssh_target("bad;rm -rf /").is_err());
    }
}

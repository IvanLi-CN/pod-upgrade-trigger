use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TaskExecutorError {
    pub code: &'static str,
    pub meta: Value,
}

impl TaskExecutorError {
    fn new(code: &'static str, meta: Value) -> Self {
        Self { code, meta }
    }
}

pub enum DispatchRequest<'a> {
    GithubWebhook { runner_unit: &'a str },
    Manual { action: &'a str },
}

pub trait TaskExecutor: Send + Sync {
    fn kind(&self) -> &'static str;

    fn dispatch(
        &self,
        task_id: &str,
        request: DispatchRequest<'_>,
    ) -> Result<(), TaskExecutorError>;

    fn stop(&self, task_id: &str, runner_unit: Option<&str>) -> Result<Value, TaskExecutorError>;

    fn force_stop(
        &self,
        task_id: &str,
        runner_unit: Option<&str>,
    ) -> Result<Value, TaskExecutorError>;
}

pub struct SystemdRunExecutor;

impl SystemdRunExecutor {
    pub fn new() -> Self {
        Self
    }

    fn dispatch_systemd_run(
        &self,
        args: Vec<String>,
        honor_snapshot: bool,
    ) -> Result<(), TaskExecutorError> {
        if honor_snapshot {
            if let Ok(snapshot) = env::var(crate::ENV_SYSTEMD_RUN_SNAPSHOT) {
                fs::write(snapshot, args.join("\n")).map_err(|e| {
                    TaskExecutorError::new(
                        "systemd-run-snapshot-write-failed",
                        crate::merge_task_meta(
                            json!({ "error": e.to_string(), "argv": args }),
                            crate::host_backend_meta(),
                        ),
                    )
                })?;
                return Ok(());
            }
        }

        let status = Command::new("systemd-run")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status();

        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(TaskExecutorError::new(
                "systemd-run-exit-nonzero",
                crate::merge_task_meta(
                    json!({ "exit": crate::exit_code_string(&status), "argv": args }),
                    crate::host_backend_meta(),
                ),
            )),
            Err(err) => Err(TaskExecutorError::new(
                "systemd-run-spawn-failed",
                crate::merge_task_meta(
                    json!({ "error": err.to_string(), "argv": args }),
                    crate::host_backend_meta(),
                ),
            )),
        }
    }

    fn dispatch_inline_run_task(&self, exe: &str, task_id: &str) -> Result<(), TaskExecutorError> {
        Command::new(exe)
            .arg("run-task")
            .arg(task_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                TaskExecutorError::new(
                    "run-task-spawn-failed",
                    crate::merge_task_meta(
                        json!({ "error": e.to_string(), "exe": exe, "task_id": task_id }),
                        crate::host_backend_meta(),
                    ),
                )
            })
    }
}

impl TaskExecutor for SystemdRunExecutor {
    fn kind(&self) -> &'static str {
        "systemd-run"
    }

    fn dispatch(
        &self,
        task_id: &str,
        request: DispatchRequest<'_>,
    ) -> Result<(), TaskExecutorError> {
        let exe = env::current_exe().map_err(|e| {
            TaskExecutorError::new(
                "current-exe-failed",
                crate::merge_task_meta(
                    json!({ "error": e.to_string() }),
                    crate::host_backend_meta(),
                ),
            )
        })?;
        let exe_str = exe.to_str().ok_or_else(|| {
            TaskExecutorError::new(
                "current-exe-invalid",
                crate::merge_task_meta(
                    json!({ "error": "invalid exe path" }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        match request {
            DispatchRequest::GithubWebhook { runner_unit } => {
                let args = crate::build_systemd_run_args(runner_unit, exe_str, task_id);
                match self.dispatch_systemd_run(args, true) {
                    Ok(()) => Ok(()),
                    Err(err) if err.code == "systemd-run-spawn-failed" => {
                        crate::log_message(&format!(
                            "warn github-dispatch-fallback executor=systemd-run code={} task_id={} running-inline",
                            err.code, task_id
                        ));
                        crate::spawn_inline_task(exe_str, task_id).map_err(|inline_err| {
                            TaskExecutorError::new(
                                "run-task-inline-spawn-failed",
                                crate::merge_task_meta(
                                    json!({
                                        "error": inline_err,
                                        "task_id": task_id,
                                        "exe": exe_str,
                                        "runner_unit": runner_unit,
                                    }),
                                    crate::host_backend_meta(),
                                ),
                            )
                        })
                    }
                    Err(err) => Err(err),
                }
            }
            DispatchRequest::Manual { .. } => {
                let mut args = Vec::new();
                args.push("--user".to_string());
                args.push("--quiet".to_string());
                for env_kv in crate::collect_run_task_env() {
                    args.push(format!("--setenv={env_kv}"));
                }
                args.push(exe_str.to_string());
                args.push("run-task".to_string());
                args.push(task_id.to_string());

                match self.dispatch_systemd_run(args, false) {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        crate::log_message(&format!(
                            "warn manual-dispatch-fallback executor=systemd-run code={} task_id={} {}",
                            err.code,
                            task_id,
                            crate::host_backend().kind().as_str()
                        ));
                        self.dispatch_inline_run_task(exe_str, task_id)
                    }
                }
            }
        }
    }

    fn stop(&self, task_id: &str, runner_unit: Option<&str>) -> Result<Value, TaskExecutorError> {
        let unit = runner_unit.ok_or_else(|| {
            TaskExecutorError::new(
                "runner-unit-missing",
                crate::merge_task_meta(
                    json!({ "task_id": task_id, "reason": "missing-runner-unit" }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        match crate::stop_task_runner_unit(unit) {
            Ok(result) if result.success() => {
                let command = format!("systemctl --user stop {unit}");
                let argv = ["systemctl", "--user", "stop", unit];
                Ok(crate::build_command_meta(
                    &command,
                    &argv,
                    &result,
                    Some(json!({ "via": "stop", "runner_unit": unit })),
                ))
            }
            Ok(result) => {
                let command = format!("systemctl --user stop {unit}");
                let argv = ["systemctl", "--user", "stop", unit];
                Err(TaskExecutorError::new(
                    "runner-stop-failed",
                    crate::build_command_meta(
                        &command,
                        &argv,
                        &result,
                        Some(json!({ "via": "stop", "runner_unit": unit })),
                    ),
                ))
            }
            Err(err) => Err(TaskExecutorError::new(
                "runner-stop-error",
                crate::merge_task_meta(
                    json!({
                        "type": "command",
                        "command": format!("systemctl --user stop {unit}"),
                        "argv": ["systemctl","--user","stop",unit],
                        "error": err,
                        "runner_unit": unit,
                    }),
                    crate::host_backend_meta(),
                ),
            )),
        }
    }

    fn force_stop(
        &self,
        task_id: &str,
        runner_unit: Option<&str>,
    ) -> Result<Value, TaskExecutorError> {
        let unit = runner_unit.ok_or_else(|| {
            TaskExecutorError::new(
                "runner-unit-missing",
                crate::merge_task_meta(
                    json!({ "task_id": task_id, "reason": "missing-runner-unit" }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        match crate::kill_task_runner_unit(unit) {
            Ok(result) if result.success() => {
                let command = format!("systemctl --user kill --signal=SIGKILL {unit}");
                let argv = ["systemctl", "--user", "kill", "--signal=SIGKILL", unit];
                Ok(crate::build_command_meta(
                    &command,
                    &argv,
                    &result,
                    Some(json!({ "via": "force-stop", "runner_unit": unit })),
                ))
            }
            Ok(result) => {
                let command = format!("systemctl --user kill --signal=SIGKILL {unit}");
                let argv = ["systemctl", "--user", "kill", "--signal=SIGKILL", unit];
                Err(TaskExecutorError::new(
                    "runner-kill-failed",
                    crate::build_command_meta(
                        &command,
                        &argv,
                        &result,
                        Some(json!({ "via": "force-stop", "runner_unit": unit })),
                    ),
                ))
            }
            Err(err) => Err(TaskExecutorError::new(
                "runner-kill-error",
                crate::merge_task_meta(
                    json!({
                        "type": "command",
                        "command": format!("systemctl --user kill --signal=SIGKILL {unit}"),
                        "argv": ["systemctl","--user","kill","--signal=SIGKILL",unit],
                        "error": err,
                        "runner_unit": unit,
                    }),
                    crate::host_backend_meta(),
                ),
            )),
        }
    }
}

pub struct LocalChildExecutor {
    exe_path: PathBuf,
    pids: Arc<Mutex<HashMap<String, u32>>>,
}

static LOCAL_CHILD_SHARED_PIDS: OnceLock<Arc<Mutex<HashMap<String, u32>>>> = OnceLock::new();

impl LocalChildExecutor {
    fn lock_pids(&self) -> std::sync::MutexGuard<'_, HashMap<String, u32>> {
        self.pids.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn shared_pids() -> Arc<Mutex<HashMap<String, u32>>> {
        Arc::clone(LOCAL_CHILD_SHARED_PIDS.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))))
    }

    pub fn from_current_exe() -> Result<Self, String> {
        let exe = env::current_exe().map_err(|e| e.to_string())?;
        Ok(Self {
            exe_path: exe,
            pids: Self::shared_pids(),
        })
    }

    pub fn with_exe_path(exe_path: PathBuf) -> Self {
        Self {
            exe_path,
            pids: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn pid_for_task(&self, task_id: &str) -> Option<u32> {
        self.lock_pids().get(task_id).copied()
    }

    fn pid_dir() -> PathBuf {
        if let Ok(raw) = env::var(crate::ENV_STATE_DIR) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("task-pids");
            }
        }
        env::temp_dir()
            .join("pod-upgrade-trigger")
            .join("task-pids")
    }

    fn sanitize_task_id_for_file(task_id: &str) -> String {
        let mut out = String::with_capacity(task_id.len());
        for ch in task_id.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                out.push(ch);
            } else {
                out.push('_');
            }
        }
        if out.is_empty() { "_".to_string() } else { out }
    }

    fn pid_file_path(task_id: &str) -> PathBuf {
        let safe = Self::sanitize_task_id_for_file(task_id);
        Self::pid_dir().join(format!("{safe}.pid"))
    }

    fn write_pid_file(task_id: &str, pid: u32) -> Result<(), std::io::Error> {
        let dir = Self::pid_dir();
        fs::create_dir_all(&dir)?;
        let path = Self::pid_file_path(task_id);
        let tmp = path.with_extension("pid.tmp");
        fs::write(&tmp, format!("{pid}\n"))?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn read_pid_file(task_id: &str) -> Result<Option<u32>, std::io::Error> {
        let path = Self::pid_file_path(task_id);
        match fs::read_to_string(&path) {
            Ok(raw) => Ok(raw.trim().parse::<u32>().ok()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn cleanup_pid_file(task_id: &str) {
        let path = Self::pid_file_path(task_id);
        if let Err(err) = fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                crate::log_message(&format!(
                    "warn local-child-pidfile-remove-failed task_id={} err={}",
                    task_id, err
                ));
            }
        }
    }

    fn build_run_task_command(&self, task_id: &str) -> Result<Command, TaskExecutorError> {
        let exe_str = self.exe_path.to_str().ok_or_else(|| {
            TaskExecutorError::new(
                "exe-path-invalid",
                crate::merge_task_meta(
                    json!({ "error": "invalid exe path", "task_id": task_id }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        let mut command = Command::new(exe_str);
        command
            .arg("--run-task")
            .arg(task_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());

        for env_kv in crate::collect_run_task_env() {
            if let Some((k, v)) = env_kv.split_once('=') {
                command.env(k, v);
            }
        }

        Ok(command)
    }

    fn send_signal(
        &self,
        task_id: &str,
        signal_name: &'static str,
        signal: i32,
    ) -> Result<Value, TaskExecutorError> {
        let pid = match self.pid_for_task(task_id) {
            Some(pid) => Some(pid),
            None => match Self::read_pid_file(task_id) {
                Ok(pid) => pid,
                Err(err) => {
                    return Err(TaskExecutorError::new(
                        "pid-read-failed",
                        crate::merge_task_meta(
                            json!({
                                "type": "signal",
                                "signal": signal_name,
                                "task_id": task_id,
                                "error": err.to_string(),
                            }),
                            crate::host_backend_meta(),
                        ),
                    ));
                }
            },
        }
        .ok_or_else(|| {
            TaskExecutorError::new(
                "pid-not-found",
                crate::merge_task_meta(
                    json!({
                        "type": "signal",
                        "signal": signal_name,
                        "task_id": task_id,
                        "error": "pid-not-found",
                    }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        let rc = unsafe { libc::kill(pid as i32, signal) };
        if rc == 0 {
            self.lock_pids().insert(task_id.to_string(), pid);
            return Ok(crate::merge_task_meta(
                json!({ "type": "signal", "signal": signal_name, "pid": pid }),
                crate::host_backend_meta(),
            ));
        }

        let os_err = std::io::Error::last_os_error();
        if os_err.raw_os_error() == Some(libc::ESRCH) {
            self.lock_pids().remove(task_id);
            Self::cleanup_pid_file(task_id);
            return Err(TaskExecutorError::new(
                "pid-not-found",
                crate::merge_task_meta(
                    json!({
                        "type": "signal",
                        "signal": signal_name,
                        "pid": pid,
                        "task_id": task_id,
                        "error": "process-not-found",
                    }),
                    crate::host_backend_meta(),
                ),
            ));
        }

        Err(TaskExecutorError::new(
            "signal-failed",
            crate::merge_task_meta(
                json!({
                    "type": "signal",
                    "signal": signal_name,
                    "pid": pid,
                    "task_id": task_id,
                    "error": os_err.to_string(),
                }),
                crate::host_backend_meta(),
            ),
        ))
    }

    fn pid_exists(pid: u32) -> bool {
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }
        let os_err = std::io::Error::last_os_error();
        match os_err.raw_os_error() {
            Some(libc::ESRCH) => false,
            Some(libc::EPERM) => true,
            _ => true,
        }
    }
}

impl TaskExecutor for LocalChildExecutor {
    fn kind(&self) -> &'static str {
        "local-child"
    }

    fn dispatch(
        &self,
        task_id: &str,
        _request: DispatchRequest<'_>,
    ) -> Result<(), TaskExecutorError> {
        if self.lock_pids().contains_key(task_id) {
            return Err(TaskExecutorError::new(
                "task-already-dispatched",
                crate::merge_task_meta(
                    json!({ "task_id": task_id, "error": "task already has an active child pid" }),
                    crate::host_backend_meta(),
                ),
            ));
        }

        if let Ok(Some(existing)) = Self::read_pid_file(task_id) {
            if Self::pid_exists(existing) {
                return Err(TaskExecutorError::new(
                    "task-already-dispatched",
                    crate::merge_task_meta(
                        json!({ "task_id": task_id, "pid": existing, "error": "task already has an active child pid" }),
                        crate::host_backend_meta(),
                    ),
                ));
            }
            // Stale pid file: remove so new dispatch can proceed.
            Self::cleanup_pid_file(task_id);
        }

        let mut command = self.build_run_task_command(task_id)?;
        let mut child = command.spawn().map_err(|e| {
            TaskExecutorError::new(
                "spawn-failed",
                crate::merge_task_meta(
                    json!({
                        "task_id": task_id,
                        "error": e.to_string(),
                        "exe": self.exe_path.to_string_lossy(),
                    }),
                    crate::host_backend_meta(),
                ),
            )
        })?;

        let pid = child.id();
        if let Err(err) = Self::write_pid_file(task_id, pid) {
            let _ = child.kill();
            return Err(TaskExecutorError::new(
                "pid-write-failed",
                crate::merge_task_meta(
                    json!({
                        "task_id": task_id,
                        "pid": pid,
                        "error": err.to_string(),
                    }),
                    crate::host_backend_meta(),
                ),
            ));
        }
        self.lock_pids().insert(task_id.to_string(), pid);

        let task_id_owned = task_id.to_string();
        let map = Arc::clone(&self.pids);
        thread::spawn(move || {
            let mut child = child;
            let mut warned = false;
            loop {
                match child.wait() {
                    Ok(_status) => {
                        let mut guard = map.lock().unwrap_or_else(|err| err.into_inner());
                        guard.remove(&task_id_owned);
                        LocalChildExecutor::cleanup_pid_file(&task_id_owned);
                        break;
                    }
                    Err(err) => {
                        if err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }

                        // If we can't wait (e.g. macOS can return ECHILD depending
                        // on SIGCHLD disposition), fall back to polling for
                        // process liveness so stop/force-stop can still find the pid.
                        if !warned {
                            warned = true;
                            crate::log_message(&format!(
                                "warn local-child-wait-error task_id={} pid={} err={} (will poll for exit)",
                                task_id_owned, pid, err
                            ));
                        }

                        if LocalChildExecutor::pid_exists(pid) {
                            thread::sleep(Duration::from_millis(200));
                            continue;
                        }

                        // Best-effort reap: if the child has exited and becomes
                        // reapable later, try once more, then forget the mapping.
                        let _ = child.wait();
                        let mut guard = map.lock().unwrap_or_else(|err| err.into_inner());
                        guard.remove(&task_id_owned);
                        LocalChildExecutor::cleanup_pid_file(&task_id_owned);
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    fn stop(&self, task_id: &str, _runner_unit: Option<&str>) -> Result<Value, TaskExecutorError> {
        self.send_signal(task_id, "SIGTERM", libc::SIGTERM)
    }

    fn force_stop(
        &self,
        task_id: &str,
        _runner_unit: Option<&str>,
    ) -> Result<Value, TaskExecutorError> {
        self.send_signal(task_id, "SIGKILL", libc::SIGKILL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_lock() -> MutexGuard<'static, ()> {
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn wait_until<F: Fn() -> bool>(timeout: Duration, check: F) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn local_child_executor_registers_and_cleans_up_pid_map() {
        let _guard = test_lock();
        let dir = tempfile::tempdir().unwrap();
        let script =
            write_executable_script(dir.path(), "child.sh", "#!/bin/sh\nsleep 0.5\nexit 0\n");

        let exec = LocalChildExecutor::with_exe_path(script);
        exec.dispatch(
            "tsk_local_child_cleanup",
            DispatchRequest::Manual { action: "test" },
        )
        .unwrap();

        assert!(
            wait_until(Duration::from_secs(1), || exec
                .pid_for_task("tsk_local_child_cleanup")
                .is_some()),
            "expected pid mapping to appear"
        );

        assert!(
            wait_until(Duration::from_secs(2), || exec
                .pid_for_task("tsk_local_child_cleanup")
                .is_none()),
            "expected pid mapping to be cleared after child exit (pid={:?} alive={})",
            exec.pid_for_task("tsk_local_child_cleanup"),
            exec.pid_for_task("tsk_local_child_cleanup")
                .map(LocalChildExecutor::pid_exists)
                .unwrap_or(false)
        );
    }

    #[test]
    fn local_child_executor_stop_sends_sigterm() {
        let _guard = test_lock();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("term.txt");
        let task_id = marker.to_string_lossy().to_string();

        let script = write_executable_script(
            dir.path(),
            "child-term.sh",
            "#!/bin/sh\nmarker=\"$2\"\ntrap 'echo TERM >\"$marker\"; exit 0' TERM\necho READY >\"$marker\"\nwhile :; do sleep 0.05; done\n",
        );

        let exec = LocalChildExecutor::with_exe_path(script);
        exec.dispatch(
            &task_id,
            DispatchRequest::GithubWebhook {
                runner_unit: "ignored",
            },
        )
        .unwrap();

        assert!(
            wait_until(Duration::from_secs(1), || exec
                .pid_for_task(&task_id)
                .is_some()),
            "expected pid mapping to appear"
        );

        assert!(
            wait_until(Duration::from_secs(2), || {
                fs::read_to_string(&marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .as_deref()
                    == Some("READY")
            }),
            "expected child to signal READY before sending SIGTERM"
        );

        exec.stop(&task_id, None).unwrap();

        assert!(
            wait_until(Duration::from_secs(2), || {
                fs::read_to_string(&marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .as_deref()
                    == Some("TERM")
            }),
            "expected SIGTERM handler to write TERM marker"
        );
        assert!(
            wait_until(Duration::from_secs(2), || exec
                .pid_for_task(&task_id)
                .is_none()),
            "expected pid mapping to be cleared after SIGTERM"
        );
    }

    #[test]
    fn local_child_executor_force_stop_sends_sigkill() {
        let _guard = test_lock();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("term.txt");
        let task_id = marker.to_string_lossy().to_string();

        let script = write_executable_script(
            dir.path(),
            "child-kill.sh",
            "#!/bin/sh\nmarker=\"$2\"\ntrap 'echo TERM >\"$marker\"; exit 0' TERM\necho READY >\"$marker\"\nwhile :; do sleep 0.05; done\n",
        );

        let exec = LocalChildExecutor::with_exe_path(script);
        exec.dispatch(&task_id, DispatchRequest::Manual { action: "test" })
            .unwrap();

        assert!(
            wait_until(Duration::from_secs(1), || exec
                .pid_for_task(&task_id)
                .is_some()),
            "expected pid mapping to appear"
        );

        assert!(
            wait_until(Duration::from_secs(2), || {
                fs::read_to_string(&marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .as_deref()
                    == Some("READY")
            }),
            "expected child to signal READY before sending SIGKILL"
        );

        exec.force_stop(&task_id, None).unwrap();

        assert!(
            wait_until(Duration::from_secs(2), || exec
                .pid_for_task(&task_id)
                .is_none()),
            "expected pid mapping to be cleared after SIGKILL"
        );

        let content = fs::read_to_string(&marker).unwrap_or_default();
        assert_eq!(
            content.trim(),
            "READY",
            "SIGKILL should not run TERM trap; marker should stay READY"
        );
    }

    #[test]
    fn local_child_executor_stop_errors_when_pid_missing() {
        let _guard = test_lock();
        let dir = tempfile::tempdir().unwrap();
        let script = write_executable_script(dir.path(), "child-exit.sh", "#!/bin/sh\nexit 0\n");

        let exec = LocalChildExecutor::with_exe_path(script);
        exec.dispatch(
            "tsk_local_child_missing",
            DispatchRequest::Manual { action: "test" },
        )
        .unwrap();

        assert!(
            wait_until(Duration::from_secs(2), || exec
                .pid_for_task("tsk_local_child_missing")
                .is_none()),
            "expected mapping to be cleared quickly"
        );

        let err = exec
            .stop("tsk_local_child_missing", None)
            .expect_err("expected missing pid error");
        assert_eq!(err.code, "pid-not-found");
    }
}

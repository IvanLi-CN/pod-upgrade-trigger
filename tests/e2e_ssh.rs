use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn sh_single_quote(raw: &str) -> String {
    if raw.is_empty() {
        "''".to_string()
    } else {
        format!("'{}'", raw.replace('\'', "'\"'\"'"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_ssh_suite() -> AnyResult<()> {
    if !env::var("PODUP_E2E_SSH")
        .ok()
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
    {
        eprintln!("[e2e_ssh] skipping (set PODUP_E2E_SSH=1 to enable)");
        return Ok(());
    }

    scenario_ssh_manual_services_and_restart().await?;
    scenario_ssh_missing_auto_update_log_dir_is_non_fatal().await?;
    Ok(())
}

async fn scenario_ssh_manual_services_and_restart() -> AnyResult<()> {
    let env = TestEnvSsh::new()?;
    env.ensure_db_initialized().await?;

    let list = env.send_request(HttpRequest::get("/api/manual/services"))?;
    assert_eq!(
        list.status,
        200,
        "GET /api/manual/services must succeed in SSH mode: {}",
        list.body_text()
    );
    let body = list.json_body()?;

    let services = body["services"].as_array().cloned().unwrap_or_default();
    assert!(
        services
            .iter()
            .any(|svc| svc.get("unit") == Some(&Value::from("podup-e2e-noop.service"))),
        "expected podup-e2e-noop.service in manual services list"
    );

    let discovered_units = body
        .pointer("/discovered/units")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        discovered_units
            .iter()
            .any(|u| u == "podup-e2e-noop.service"),
        "expected podup-e2e-noop.service in discovered.units (from remote PODUP_CONTAINER_DIR)"
    );

    let trigger_body = json!({
        "dry_run": false,
        "caller": "e2e-ssh",
        "reason": "noop-restart",
    });
    let trigger = env.send_request(
        HttpRequest::post("/api/manual/services/podup-e2e-noop.service")
            .header("content-type", "application/json")
            .header("x-podup-csrf", "1")
            .body(trigger_body.to_string().into_bytes()),
    )?;
    assert_eq!(
        trigger.status, 202,
        "manual service trigger should be accepted"
    );
    let trigger_json = trigger.json_body()?;
    let task_id = trigger_json["task_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !task_id.is_empty(),
        "manual service trigger must return task_id"
    );

    let task = env.wait_for_task_terminal(&task_id, Duration::from_secs(30))?;
    assert_eq!(
        task["status"],
        Value::from("succeeded"),
        "noop service restart must succeed: {}",
        task
    );

    let logs = task["logs"].as_array().cloned().unwrap_or_default();
    let run_log = logs
        .iter()
        .find(|entry| entry.get("action") == Some(&Value::from("manual-service-run")))
        .expect("manual-service-run log entry should exist");
    let meta = run_log.get("meta").cloned().unwrap_or(Value::Null);
    assert_eq!(
        meta.get("host_backend"),
        Some(&Value::from("ssh")),
        "manual-service-run must record host_backend=ssh meta: {}",
        meta
    );
    assert_eq!(
        meta.get("task_executor"),
        Some(&Value::from("local-child")),
        "SSH mode should default to local-child executor meta: {}",
        meta
    );

    Ok(())
}

async fn scenario_ssh_missing_auto_update_log_dir_is_non_fatal() -> AnyResult<()> {
    let env = TestEnvSsh::new()?;
    env.ensure_db_initialized().await?;

    let remote_log_dir = env.remote_log_dir.clone();
    assert!(
        !env.remote_path_exists(&remote_log_dir)?,
        "test requires PODUP_AUTO_UPDATE_LOG_DIR to be absent on SSH target: {}",
        remote_log_dir
    );

    let body = json!({
        "dry_run": false,
        "caller": "e2e-ssh",
        "reason": "missing-auto-update-log-dir",
    });
    let response = env.send_request(
        HttpRequest::post("/api/manual/auto-update/run")
            .header("content-type", "application/json")
            .header("x-podup-csrf", "1")
            .body(body.to_string().into_bytes()),
    )?;

    assert!(
        matches!(response.status, 202 | 500),
        "manual auto-update run should not hang (status={} body={})",
        response.status,
        response.body_text()
    );
    if response.status == 202 {
        let json = response.json_body()?;
        let task_id = json["task_id"].as_str().unwrap_or_default().to_string();
        assert!(
            !task_id.is_empty(),
            "manual auto-update run must return task_id on 202"
        );
        let _ = env.wait_for_task_terminal(&task_id, Duration::from_secs(60))?;
    }

    assert!(
        !env.remote_path_exists(&remote_log_dir)?,
        "backend must not create PODUP_AUTO_UPDATE_LOG_DIR on SSH target: {}",
        remote_log_dir
    );

    Ok(())
}

struct TestEnvSsh {
    #[allow(dead_code)]
    temp: TempDir,
    state_dir: PathBuf,
    db_path: PathBuf,
    debug_payload: PathBuf,
    manual_token: String,
    github_secret: String,
    bin_path: PathBuf,
    ssh_target: String,
    remote_container_dir: String,
    remote_log_dir: String,
}

impl TestEnvSsh {
    fn new() -> AnyResult<Self> {
        let ssh_target = env::var("PODUP_SSH_TARGET")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "PODUP_SSH_TARGET is required"))?;
        let remote_container_dir = env::var("PODUP_CONTAINER_DIR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::Other, "PODUP_CONTAINER_DIR is required")
            })?;
        let remote_log_dir = env::var("PODUP_AUTO_UPDATE_LOG_DIR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    "PODUP_AUTO_UPDATE_LOG_DIR is required",
                )
            })?;

        let temp = TempDir::new()?;
        let root = temp.path().to_path_buf();
        let state_dir = root.join("state");
        std::fs::create_dir_all(&state_dir)?;

        let db_path = root.join("db/pod-upgrade-trigger.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(&db_path)?;

        let debug_payload = state_dir.join("last_payload.bin");
        let manual_token = "e2e-ssh-manual".to_string();
        let github_secret = "e2e-ssh-github-secret".to_string();
        let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_pod-upgrade-trigger"));

        Ok(Self {
            temp,
            state_dir,
            db_path,
            debug_payload,
            manual_token,
            github_secret,
            bin_path,
            ssh_target,
            remote_container_dir,
            remote_log_dir,
        })
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin_path);
        cmd.env("PODUP_STATE_DIR", &self.state_dir);
        cmd.env("PODUP_DB_URL", self.db_url());
        cmd.env("PODUP_TOKEN", &self.manual_token);
        cmd.env("PODUP_GH_WEBHOOK_SECRET", &self.github_secret);
        cmd.env("PODUP_MANUAL_UNITS", "podup-e2e-noop.service");
        cmd.env("PODUP_DEBUG_PAYLOAD_PATH", &self.debug_payload);
        cmd.env("PODUP_ENV", "test");
        cmd.env("PODUP_DEV_OPEN_ADMIN", "1");
        cmd.env("PODUP_AUDIT_SYNC", "1");
        cmd.env("PODUP_SCHEDULER_MIN_INTERVAL_SECS", "0");
        cmd.env("PODUP_SSH_TARGET", &self.ssh_target);
        cmd.env("PODUP_CONTAINER_DIR", &self.remote_container_dir);
        cmd.env("PODUP_AUTO_UPDATE_LOG_DIR", &self.remote_log_dir);
        cmd.stdin(Stdio::null());
        cmd
    }

    fn db_url(&self) -> String {
        format!("sqlite://{}", self.db_path.display())
    }

    fn send_request(&self, request: HttpRequest) -> AnyResult<HttpResponse> {
        let mut cmd = self.command();
        cmd.arg("server");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        {
            let mut stdin = child.stdin.take().expect("stdin available");
            stdin.write_all(&request.into_bytes())?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "server command failed: {} stderr: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
            )
            .into());
        }
        HttpResponse::parse(&output.stdout)
    }

    async fn ensure_db_initialized(&self) -> AnyResult<()> {
        let mut cmd = self.command();
        cmd.arg("prune-state").arg("--dry-run");
        let output = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "prune-state failed: {} stderr: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
            )
            .into());
        }
        Ok(())
    }

    fn wait_for_task_terminal(&self, task_id: &str, timeout: Duration) -> AnyResult<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let detail = self.send_request(HttpRequest::get(&format!("/api/tasks/{task_id}")))?;
            if detail.status != 200 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("task detail request failed: {}", detail.body_text()),
                )
                .into());
            }
            let body = detail.json_body()?;
            match body["status"].as_str().unwrap_or("unknown") {
                "succeeded" | "failed" | "cancelled" | "skipped" => return Ok(body),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("task did not reach terminal state in time: {task_id}"),
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn remote_path_exists(&self, path: &str) -> AnyResult<bool> {
        let status = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ConnectionAttempts=1",
                &self.ssh_target,
                "--",
                "bash",
                "-lc",
                &format!("test -e {} >/dev/null 2>&1", sh_single_quote(path)),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn get(path: &str) -> Self {
        Self::new("GET", path)
    }

    fn post(path: &str) -> Self {
        Self::new("POST", path)
    }

    fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            headers: vec![("host".into(), "localhost".into())],
            body: Vec::new(),
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut headers = self.headers;
        let has_host = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"));
        if !has_host {
            headers.push(("Host".into(), "localhost".into()));
        }

        let mut lines = Vec::new();
        lines.push(format!("{} {} HTTP/1.1\r\n", self.method, self.path));
        let mut has_content_length = false;
        for (name, value) in &headers {
            if name.eq_ignore_ascii_case("content-length") {
                has_content_length = true;
            }
            lines.push(format!("{}: {}\r\n", name, value));
        }
        if !self.body.is_empty() && !has_content_length {
            lines.push(format!("Content-Length: {}\r\n", self.body.len()));
        }
        lines.push("Connection: close\r\n".into());
        lines.push("\r\n".into());

        let mut payload: Vec<u8> = lines.into_iter().flat_map(|s| s.into_bytes()).collect();
        payload.extend_from_slice(&self.body);
        payload
    }
}

struct HttpResponse {
    status: u16,
    #[allow(dead_code)]
    reason: String,
    #[allow(dead_code)]
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn parse(raw: &[u8]) -> AnyResult<Self> {
        let split = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "invalid HTTP response"))?;
        let (head, body) = raw.split_at(split + 4);
        let head_str = String::from_utf8_lossy(head);
        let mut lines = head_str.split("\r\n");
        let status_line = lines
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing status line"))?;
        let mut status_parts = status_line.splitn(3, ' ');
        let _http = status_parts.next().unwrap_or("HTTP/1.1");
        let status = status_parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing status code"))?
            .parse::<u16>()?;
        let reason = status_parts.next().unwrap_or("").to_string();

        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }

        Ok(Self {
            status,
            reason,
            headers,
            body: body.to_vec(),
        })
    }

    fn json_body(&self) -> AnyResult<Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

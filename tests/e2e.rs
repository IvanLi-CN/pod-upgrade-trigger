use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type HmacSha256 = Hmac<Sha256>;

#[tokio::test(flavor = "multi_thread")]
async fn e2e_full_suite() -> AnyResult<()> {
    scenario_auto_discovery().await?;
    scenario_webhook_auto_discovery_toggle().await?;
    scenario_health_db_error().await?;
    scenario_github_webhook().await?;
    scenario_rate_limit_and_prune().await?;
    scenario_task_prune_retention().await?;
    scenario_manual_api().await?;
    scenario_scheduler_loop().await?;
    scenario_events_task_filter().await?;
    scenario_error_paths().await?;
    scenario_static_assets().await?;
    scenario_cli_maintenance().await?;
    scenario_http_server().await?;
    Ok(())
}

async fn scenario_auto_discovery() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;

    let container_dir = env.state_dir.join("containers/systemd");
    fs::create_dir_all(&container_dir)?;
    fs::write(
        container_dir.join("svc-gamma.container"),
        b"[Container]\nImage=example\nAutoupdate=registry",
    )?;
    fs::write(
        container_dir.join("svc-delta.service"),
        b"[Unit]\nDescription=dummy",
    )?;

    let services = env.send_request_with_env(HttpRequest::get("/api/manual/services"), |cmd| {
        cmd.env(
            "PODUP_CONTAINER_DIR",
            env.state_dir.join("containers/systemd"),
        );
    })?;

    assert_eq!(services.status, 200);
    let body = services.json_body()?;
    let discovered = body["discovered"]["units"].as_array().unwrap();
    assert_eq!(discovered.len(), 2);
    let sources: Vec<_> = body["services"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|svc| svc["source"] == Value::from("discovered"))
        .collect();
    assert_eq!(sources.len(), 2);

    Ok(())
}

async fn scenario_webhook_auto_discovery_toggle() -> AnyResult<()> {
    let env = TestEnv::new()?;

    let container_dir = env.state_dir.join("containers/systemd");
    fs::create_dir_all(&container_dir)?;
    fs::write(
        container_dir.join("svc-gamma.container"),
        b"[Container]\nImage=example\nAutoupdate=registry",
    )?;
    fs::write(
        container_dir.join("svc-delta.service"),
        b"[Unit]\nDescription=dummy",
    )?;

    // Auto-discovery disabled (default): webhooks list should only include manual/env units.
    let disabled = env.send_request_with_env(HttpRequest::get("/api/webhooks/status"), |cmd| {
        cmd.env("PODUP_CONTAINER_DIR", &container_dir);
    })?;
    assert_eq!(disabled.status, 200);
    let body = disabled.json_body()?;
    let units = body["units"].as_array().unwrap();
    let unit_names: Vec<String> = units
        .iter()
        .filter_map(|u| {
            u.get("unit")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        !unit_names.iter().any(|u| u == "svc-gamma.service"),
        "svc-gamma.service should not be present when PODUP_AUTO_DISCOVER is disabled",
    );
    assert!(
        !unit_names.iter().any(|u| u == "svc-delta.service"),
        "svc-delta.service should not be present when PODUP_AUTO_DISCOVER is disabled",
    );

    // Auto-discovery enabled: webhooks list should include discovered units.
    let enabled = env.send_request_with_env(HttpRequest::get("/api/webhooks/status"), |cmd| {
        cmd.env("PODUP_CONTAINER_DIR", &container_dir);
        cmd.env("PODUP_AUTO_DISCOVER", "1");
    })?;
    assert_eq!(enabled.status, 200);
    let body = enabled.json_body()?;
    let units = body["units"].as_array().unwrap();
    let unit_names: Vec<String> = units
        .iter()
        .filter_map(|u| {
            u.get("unit")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        unit_names.iter().any(|u| u == "svc-gamma.service"),
        "svc-gamma.service should be present when PODUP_AUTO_DISCOVER is enabled",
    );
    assert!(
        unit_names.iter().any(|u| u == "svc-delta.service"),
        "svc-delta.service should be present when PODUP_AUTO_DISCOVER is enabled",
    );

    Ok(())
}

async fn scenario_health_db_error() -> AnyResult<()> {
    let env = TestEnv::new()?;
    let response = env.send_request_with_env(HttpRequest::get("/health"), |cmd| {
        cmd.env("PODUP_DB_URL", "postgres://forbidden/uri");
    })?;

    assert_eq!(response.status, 503);
    let json = response.json_body()?;
    assert_eq!(json["status"], Value::from("degraded"));
    let issues = json["issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue["component"] == Value::from("database"))
    );

    Ok(())
}

async fn scenario_github_webhook() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;

    let payload = github_registry_payload("koha", "svc-alpha", "main");
    let signature = env.github_signature(&payload);
    let response = env.send_request(
        HttpRequest::post("/github-package-update/svc-alpha")
            .header("x-github-event", "registry_package")
            .header("x-github-delivery", "delivery-42")
            .header("x-hub-signature-256", &signature)
            .body(payload.clone()),
    )?;
    assert_eq!(
        response.status,
        202,
        "github webhook accepted: {}",
        response.body_text()
    );

    let log_lines = env.read_mock_log()?;
    assert!(
        log_lines.iter().any(|line| line
            .contains("systemd-run --user --collect --quiet --unit=webhook-task-delivery-42")),
        "systemd-run dispatch recorded"
    );
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("podman pull ghcr.io/koha/svc-alpha:main")),
        "podman pull recorded"
    );
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("systemctl --user restart svc-alpha.service")),
        "systemctl restart recorded"
    );
    let pool = env.connect_db().await?;
    let webhook_event = env
        .fetch_events(&pool)
        .await?
        .into_iter()
        .find(|event| event.action == "github-webhook")
        .expect("webhook action stored");
    assert_eq!(webhook_event.status, 202);
    assert_eq!(
        webhook_event.meta.get("unit").and_then(|v| v.as_str()),
        Some("svc-alpha.service")
    );

    let github_tokens: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rate_limit_tokens WHERE scope = 'github-image'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(github_tokens, 1);

    let lock_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM image_locks")
        .fetch_one(&pool)
        .await?;
    assert_eq!(lock_count, 0);

    Ok(())
}

async fn scenario_rate_limit_and_prune() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;
    env.ensure_db_initialized().await?;

    let pool = env.connect_db().await?;
    let now = current_unix_secs() as i64;
    for _ in 0..3 {
        sqlx::query(
            "INSERT INTO rate_limit_tokens (scope, bucket, ts) VALUES ('manual', 'manual-auto-update', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await?;
    }

    let rate_limited = env.send_request(HttpRequest::get("/auto-update?token=e2e-manual"))?;
    assert_eq!(rate_limited.status, 429);

    sqlx::query("UPDATE rate_limit_tokens SET ts = ts - 200000")
        .execute(&pool)
        .await?;

    let mut prune_cmd = env.command();
    prune_cmd
        .arg("prune-state")
        .arg("--max-age-hours")
        .arg("48");
    let prune_output = env.run_command(prune_cmd)?;
    assert!(prune_output.status.success());

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rate_limit_tokens WHERE scope = 'manual'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining, 0);

    let success = env.send_request(HttpRequest::get("/auto-update?token=e2e-manual"))?;
    assert_eq!(
        success.status,
        202,
        "manual auto-update after prune: {}",
        success.body_text()
    );

    let log_lines = env.read_mock_log()?;
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("systemctl --user start podman-auto-update.service"))
    );

    let events = env.fetch_events(&pool).await?;
    assert!(
        events
            .iter()
            .any(|row| row.action == "manual-auto-update" && row.status == 429)
    );
    assert!(
        events
            .iter()
            .any(|row| row.action == "manual-auto-update" && row.status == 202)
    );

    Ok(())
}

async fn scenario_task_prune_retention() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.ensure_db_initialized().await?;

    let pool = env.connect_db().await?;
    let now = current_unix_secs() as i64;
    let old_finished = now - 2 * 3600;
    let recent_finished = now;

    // Old succeeded task (should be pruned).
    sqlx::query(
        "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, summary, meta, trigger_source) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("old-succeeded")
    .bind("manual")
    .bind("succeeded")
    .bind(old_finished)
    .bind(old_finished)
    .bind(old_finished)
    .bind("old succeeded task")
    .bind("{}")
    .bind("test")
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO task_units (task_id, unit, status) VALUES (?, ?, ?)",
    )
    .bind("old-succeeded")
    .bind("svc-old.service")
    .bind("succeeded")
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO task_logs (task_id, ts, level, action, status, summary) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("old-succeeded")
    .bind(old_finished)
    .bind("info")
    .bind("test-old")
    .bind("succeeded")
    .bind("old task log")
    .execute(&pool)
    .await?;

    // Recent succeeded task (should be kept).
    sqlx::query(
        "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, summary, meta, trigger_source) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("recent-succeeded")
    .bind("manual")
    .bind("succeeded")
    .bind(recent_finished)
    .bind(recent_finished)
    .bind(recent_finished)
    .bind("recent succeeded task")
    .bind("{}")
    .bind("test")
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO task_units (task_id, unit, status) VALUES (?, ?, ?)",
    )
    .bind("recent-succeeded")
    .bind("svc-recent.service")
    .bind("succeeded")
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO task_logs (task_id, ts, level, action, status, summary) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("recent-succeeded")
    .bind(recent_finished)
    .bind("info")
    .bind("test-recent")
    .bind("succeeded")
    .bind("recent task log")
    .execute(&pool)
    .await?;

    // Running task (non-terminal status, should be kept).
    sqlx::query(
        "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, summary, meta, trigger_source) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("running-task")
    .bind("manual")
    .bind("running")
    .bind(now)
    .bind(now)
    .bind(Option::<i64>::None)
    .bind("running task")
    .bind("{}")
    .bind("test")
    .execute(&pool)
    .await?;

    // Terminal status without finished_at (should be kept).
    sqlx::query(
        "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, summary, meta, trigger_source) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("succeeded-no-finished")
    .bind("manual")
    .bind("succeeded")
    .bind(now)
    .bind(now)
    .bind(Option::<i64>::None)
    .bind("succeeded without finished_at")
    .bind("{}")
    .bind("test")
    .execute(&pool)
    .await?;

    // Run prune-state with a 1 hour task retention window so only the old task is pruned.
    let mut prune_cmd = env.command();
    prune_cmd.arg("prune-state");
    prune_cmd.env("PODUP_TASK_RETENTION_SECS", "3600");
    let prune_output = env.run_command(prune_cmd)?;
    assert!(
        prune_output.status.success(),
        "prune-state task prune failed: status={} stdout={} stderr={}",
        prune_output.status,
        prune_output.stdout,
        prune_output.stderr
    );

    // Old succeeded task and its units/logs should be gone.
    let remaining_old_tasks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE task_id = 'old-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_old_tasks, 0);

    let remaining_old_units: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_units WHERE task_id = 'old-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_old_units, 0);

    let remaining_old_logs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_logs WHERE task_id = 'old-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_old_logs, 0);

    // Recent succeeded task and its units/logs should still exist.
    let remaining_recent_tasks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE task_id = 'recent-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_recent_tasks, 1);

    let remaining_recent_units: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_units WHERE task_id = 'recent-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_recent_units, 1);

    let remaining_recent_logs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_logs WHERE task_id = 'recent-succeeded'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_recent_logs, 1);

    // Running and terminal-without-finished tasks should remain.
    let remaining_running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE task_id = 'running-task'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_running, 1);

    let remaining_no_finished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE task_id = 'succeeded-no-finished'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(remaining_no_finished, 1);

    Ok(())
}

async fn scenario_manual_api() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;
    // Warm discovery so subsequent checks are not polluted by podman logs.
    let _ = env.send_request(HttpRequest::get("/api/manual/services"))?;
    env.clear_mock_log()?;
    let trigger_body = json!({
        "token": env.manual_token(),
        "all": true,
        "dry_run": true,
        "caller": "ci",
        "reason": "validation"
    });
    let trigger = env.send_request(
        HttpRequest::post("/api/manual/trigger")
            .header("content-type", "application/json")
            .body(trigger_body.to_string().into_bytes()),
    )?;
    assert_eq!(trigger.status, 202);
    let trigger_json = trigger.json_body()?;
    assert_eq!(trigger_json["dry_run"], Value::from(true));
    assert_eq!(trigger_json["triggered"].as_array().unwrap().len(), 3);
    // Manual trigger response should echo a non-empty request_id for UI navigation.
    let trigger_request_id = trigger_json["request_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !trigger_request_id.is_empty(),
        "manual trigger response must include request_id"
    );
    let non_discovery: Vec<_> = env
        .read_mock_log()?
        .into_iter()
        .filter(|line| {
            !line.contains("podman auto-update --dry-run")
                && !line.contains("podman ps --filter label=io.containers.autoupdate")
                && !line.contains("podman ps -a --filter label=io.containers.autoupdate")
        })
        .collect();
    assert!(non_discovery.is_empty());

    env.clear_mock_log()?;
    let service_body = json!({
        "token": env.manual_token(),
        "dry_run": false,
        "image": "ghcr.io/koha/runner:main",
        "caller": "user",
        "reason": "rollout"
    });
    let service = env.send_request(
        HttpRequest::post("/api/manual/services/svc-beta")
            .header("content-type", "application/json")
            .body(service_body.to_string().into_bytes()),
    )?;
    assert_eq!(service.status, 202);
    let service_json = service.json_body()?;
    assert_eq!(service_json["status"], Value::from("pending"));
    // Per-service trigger should also return a non-empty request_id.
    let service_request_id = service_json["request_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !service_request_id.is_empty(),
        "manual service response must include request_id"
    );

    let log_lines = env.read_mock_log()?;
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("podman pull ghcr.io/koha/runner:main"))
    );
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("systemctl --user restart svc-beta.service"))
    );

    let pool = env.connect_db().await?;
    let events = env.fetch_events(&pool).await?;
    assert!(events.iter().any(|row| row.action == "manual-trigger"));
    assert!(events.iter().any(|row| row.action == "manual-service"));

    Ok(())
}

async fn scenario_scheduler_loop() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;
    let mut cmd = env.command();
    cmd.arg("scheduler")
        .arg("--interval")
        .arg("1")
        .arg("--max-iterations")
        .arg("2");
    let output = env.run_command(cmd)?;
    assert!(
        output.status.success(),
        "scheduler failed: status={} stdout={} stderr={}",
        output.status,
        output.stdout,
        output.stderr
    );

    let log_lines = env.read_mock_log()?;
    assert!(
        log_lines
            .iter()
            .filter(|line| line.contains("systemctl --user start podman-auto-update.service"))
            .count()
            >= 2
    );

    let pool = env.connect_db().await?;
    let scheduler_events: Vec<_> = env
        .fetch_events(&pool)
        .await?
        .into_iter()
        .filter(|row| row.action == "scheduler")
        .collect();
    assert_eq!(scheduler_events.len(), 2);
    Ok(())
}

async fn scenario_events_task_filter() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.ensure_db_initialized().await?;

    let mut trigger_cmd = env.command();
    trigger_cmd.arg("trigger-units").arg("svc-alpha.service");
    let trigger_output = env.run_command(trigger_cmd)?;
    assert!(
        trigger_output.status.success(),
        "trigger-units svc-alpha.service failed: status={} stdout={} stderr={}",
        trigger_output.status,
        trigger_output.stdout,
        trigger_output.stderr
    );

    let pool = env.connect_db().await?;
    let events = env.fetch_events(&pool).await?;
    let task_id = events
        .iter()
        .find_map(|row| row.meta.get("task_id").and_then(|v| v.as_str()))
        .unwrap_or_default()
        .to_string();
    assert!(
        !task_id.is_empty(),
        "cli-trigger events should include a task_id in meta"
    );

    let path = format!("/api/events?task_id={task_id}");
    let response = env.send_request(HttpRequest::get(&path))?;
    assert_eq!(response.status, 200, "/api/events?task_id status");
    let body = response.json_body()?;
    let events = body["events"].as_array().cloned().unwrap_or_default();
    assert!(
        !events.is_empty(),
        "/api/events?task_id filter should return at least one event"
    );

    Ok(())
}

async fn scenario_error_paths() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;
    let payload = github_registry_payload("koha", "svc-alpha", "main");
    let signature = env.github_signature(&payload);
    let request = HttpRequest::post("/github-package-update/svc-alpha")
        .header("x-github-event", "registry_package")
        .header("x-github-delivery", "bad-task")
        .header("x-hub-signature-256", &signature)
        .body(payload.clone());
    let response = env.send_request_with_env(request, |cmd| {
        cmd.env("MOCK_SYSTEMD_RUN_FAIL", "webhook-task-bad-task");
    })?;
    assert_eq!(response.status, 500);

    env.clear_mock_log()?;
    let payload2 = github_registry_payload("koha", "svc-beta", "main");
    let signature2 = env.github_signature(&payload2);
    let response2 = env.send_request_with_env(
        HttpRequest::post("/github-package-update/svc-beta")
            .header("x-github-event", "registry_package")
            .header("x-github-delivery", "podman-fail")
            .header("x-hub-signature-256", &signature2)
            .body(payload2.clone()),
        |cmd| {
            cmd.env("MOCK_PODMAN_FAIL", "1");
        },
    )?;
    assert_eq!(response2.status, 202);
    let log_lines = env.read_mock_log()?;
    assert!(log_lines.iter().any(|line| line.contains("podman pull")));
    assert!(
        log_lines
            .iter()
            .all(|line| !line.contains("systemctl --user restart svc-beta.service"))
    );

    let invalid = HttpRequest::post("/github-package-update/svc-alpha")
        .header("x-github-event", "registry_package")
        .header("x-github-delivery", "invalid-signature")
        .header("x-hub-signature-256", "sha256=deadbeef")
        .body(payload.clone());
    let invalid_resp = env.send_request(invalid)?;
    assert_eq!(invalid_resp.status, 401);
    assert!(env.last_payload_dump().exists());
    let dumped = fs::read(env.last_payload_dump())?;
    assert_eq!(dumped, payload);

    let pool = env.connect_db().await?;
    let events = env.fetch_events(&pool).await?;
    assert!(
        events
            .iter()
            .any(|row| row.action == "github-webhook" && row.status == 500)
    );
    assert!(
        events
            .iter()
            .any(|row| row.action == "github-webhook" && row.status == 202)
    );
    assert!(
        events
            .iter()
            .any(|row| row.action == "github-webhook" && row.status == 401)
    );

    Ok(())
}

async fn scenario_static_assets() -> AnyResult<()> {
    let env = TestEnv::new()?;
    let health = env.send_request(HttpRequest::get("/health"))?;
    assert_eq!(health.status, 200);
    assert!(String::from_utf8_lossy(&health.body).contains("ok"));

    let index = env.send_request(HttpRequest::get("/"))?;
    assert_eq!(index.status, 200);
    assert!(String::from_utf8_lossy(&index.body).contains("Hello from e2e dist"));

    let asset = env.send_request(HttpRequest::get("/assets/app.js"))?;
    assert_eq!(asset.status, 200);
    assert!(String::from_utf8_lossy(&asset.body).contains("window.__E2E__"));

    Ok(())
}

async fn scenario_cli_maintenance() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.clear_mock_log()?;

    let mut trigger_alpha = env.command();
    trigger_alpha.arg("trigger-units").arg("svc-alpha.service");
    let alpha_output = env.run_command(trigger_alpha)?;
    assert!(
        alpha_output.status.success(),
        "trigger-units svc-alpha.service failed: status={} stdout={} stderr={}",
        alpha_output.status,
        alpha_output.stdout,
        alpha_output.stderr
    );

    let mut trigger_manual = env.command();
    trigger_manual
        .arg("trigger-units")
        .arg("podman-auto-update.service");
    let manual_output = env.run_command(trigger_manual)?;
    assert!(
        manual_output.status.success(),
        "trigger-units podman-auto-update.service failed: status={} stdout={} stderr={}",
        manual_output.status,
        manual_output.stdout,
        manual_output.stderr
    );

    let log_lines = env.read_mock_log()?;
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("systemctl --user restart svc-alpha.service"))
    );
    assert!(
        log_lines
            .iter()
            .any(|line| line.contains("systemctl --user start podman-auto-update.service"))
    );

    env.clear_mock_log()?;
    let mut trigger_all = env.command();
    trigger_all
        .arg("trigger-all")
        .arg("--dry-run")
        .arg("--caller")
        .arg("ops")
        .arg("--reason")
        .arg("smoke");
    let trigger_all_output = env.run_command(trigger_all)?;
    assert!(
        trigger_all_output.status.success(),
        "trigger-all failed: status={} stdout={} stderr={}",
        trigger_all_output.status,
        trigger_all_output.stdout,
        trigger_all_output.stderr
    );
    let non_discovery: Vec<_> = env
        .read_mock_log()?
        .into_iter()
        .filter(|line| {
            !line.contains("podman auto-update --dry-run")
                && !line.contains("podman ps --filter label=io.containers.autoupdate")
                && !line.contains("podman ps -a --filter label=io.containers.autoupdate")
        })
        .collect();
    assert!(non_discovery.is_empty());

    let mut prune_cmd = env.command();
    prune_cmd
        .arg("prune-state")
        .arg("--max-age-hours")
        .arg("1")
        .arg("--dry-run");
    let prune_output = env.run_command(prune_cmd)?;
    assert!(
        prune_output.status.success(),
        "prune-state --dry-run failed: status={} stdout={} stderr={}",
        prune_output.status,
        prune_output.stdout,
        prune_output.stderr
    );

    let pool = env.connect_db().await?;
    let cli_events: Vec<_> = env
        .fetch_events(&pool)
        .await?
        .into_iter()
        .filter(|row| row.action == "cli-trigger")
        .collect();
    assert!(cli_events.len() >= 3);

    Ok(())
}

async fn scenario_http_server() -> AnyResult<()> {
    let env = TestEnv::new()?;
    env.ensure_db_initialized().await?;

    let addr = "127.0.0.1:25111";

    let mut cmd = env.command();
    cmd.arg("http-server");
    cmd.env("PODUP_HTTP_ADDR", addr);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn()?;

    // Give the server a short window to start listening.
    let mut last_err: Option<io::Error> = None;
    for _ in 0..20 {
        match TcpStream::connect(addr) {
            Ok(mut stream) => {
                let request = HttpRequest::get("/health").into_bytes();
                stream.write_all(&request)?;
                // Signal end of request body.
                let _ = stream.shutdown(std::net::Shutdown::Write);

                let mut buf = Vec::new();
                stream.read_to_end(&mut buf)?;
                let response = HttpResponse::parse(&buf)?;
                assert_eq!(response.status, 200, "http-server /health status");
                assert!(
                    response.body_text().contains("ok"),
                    "http-server /health body: {}",
                    response.body_text()
                );

                child.kill().ok();
                child.wait().ok();
                return Ok(());
            }
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "http-server did not start on {addr} in time: last_err={:?}",
        last_err
    )
    .into())
}

fn github_registry_payload(owner: &str, name: &str, tag: &str) -> Vec<u8> {
    json!({
        "registry_package": {
            "package_type": "container",
            "name": name,
            "namespace": owner,
            "package_version": {
                "metadata": {
                    "container": {
                        "tags": [tag]
                    }
                }
            }
        },
        "registry": {
            "host": "ghcr.io"
        }
    })
    .to_string()
    .into_bytes()
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

struct TestEnv {
    #[allow(dead_code)]
    temp: TempDir,
    state_dir: PathBuf,
    db_path: PathBuf,
    debug_payload: PathBuf,
    manual_token: String,
    github_secret: String,
    bin_path: PathBuf,
    mock_log: PathBuf,
    path_override: String,
}

impl TestEnv {
    fn new() -> AnyResult<Self> {
        let temp = TempDir::new()?;
        let root = temp.path().to_path_buf();
        let state_dir = root.join("state");
        fs::create_dir_all(&state_dir)?;
        let db_path = root.join("db/pod-upgrade-trigger.db");
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(&db_path)?;
        let web_dist = state_dir.join("web/dist");
        fs::create_dir_all(web_dist.join("assets"))?;
        fs::write(
            web_dist.join("index.html"),
            "<!doctype html><html><body>Hello from e2e dist</body></html>",
        )?;
        fs::write(
            web_dist.join("assets/app.js"),
            "window.__E2E__ = true; console.log('ok');",
        )?;
        let manual_token = "e2e-manual".to_string();
        let github_secret = "e2e-github-secret".to_string();
        let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_pod-upgrade-trigger"));
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mock_dir = manifest_dir.join("tests/mock-bin");
        let mock_log = mock_dir.join("log.txt");
        fs::write(&mock_log, b"")?;
        let upstream_path = env::var("PATH").unwrap_or_default();
        let path_override = format!("{}:{}", mock_dir.display(), upstream_path);
        let debug_payload = state_dir.join("last_payload.bin");
        Ok(Self {
            temp,
            state_dir,
            db_path,
            debug_payload,
            manual_token,
            github_secret,
            bin_path,
            mock_log,
            path_override,
        })
    }

    fn clear_mock_log(&self) -> AnyResult<()> {
        fs::write(&self.mock_log, b"")?;
        Ok(())
    }

    fn read_mock_log(&self) -> AnyResult<Vec<String>> {
        if !self.mock_log.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.mock_log)?;
        Ok(content
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect())
    }

    fn manual_token(&self) -> &str {
        &self.manual_token
    }

    fn last_payload_dump(&self) -> &Path {
        &self.debug_payload
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin_path);
        cmd.env("PODUP_STATE_DIR", &self.state_dir);
        cmd.env("PODUP_DB_URL", self.db_url());
        cmd.env("PODUP_TOKEN", &self.manual_token);
        cmd.env("PODUP_GH_WEBHOOK_SECRET", &self.github_secret);
        cmd.env("PODUP_MANUAL_UNITS", "svc-alpha.service,svc-beta.service");
        cmd.env("PODUP_DEBUG_PAYLOAD_PATH", &self.debug_payload);
        cmd.env("PODUP_ENV", "test");
        cmd.env("PODUP_AUDIT_SYNC", "1");
        cmd.env("PODUP_SCHEDULER_MIN_INTERVAL_SECS", "0");
        cmd.env("PATH", &self.path_override);
        cmd.stdin(Stdio::null());
        cmd
    }

    fn db_url(&self) -> String {
        format!("sqlite://{}", self.db_path.display())
    }

    fn run_command(&self, mut cmd: Command) -> AnyResult<CommandResult> {
        let output = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
        Ok(CommandResult {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn github_signature(&self, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(self.github_secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={:x}", mac.finalize().into_bytes())
    }

    fn send_request(&self, request: HttpRequest) -> AnyResult<HttpResponse> {
        self.send_request_with_env(request, |_| {})
    }

    fn send_request_with_env<F>(
        &self,
        request: HttpRequest,
        configure: F,
    ) -> AnyResult<HttpResponse>
    where
        F: FnOnce(&mut Command),
    {
        let mut cmd = self.command();
        cmd.arg("server");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&mut cmd);
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

    async fn connect_db(&self) -> AnyResult<SqlitePool> {
        Ok(SqlitePool::connect(&self.db_url()).await?)
    }

    async fn ensure_db_initialized(&self) -> AnyResult<()> {
        let mut cmd = self.command();
        cmd.arg("prune-state").arg("--dry-run");
        let _ = self.run_command(cmd)?;
        Ok(())
    }

    async fn fetch_events(&self, pool: &SqlitePool) -> AnyResult<Vec<EventRow>> {
        let rows = sqlx::query("SELECT action, status, meta FROM event_log ORDER BY id")
            .fetch_all(pool)
            .await?;
        let mut events = Vec::new();
        for row in rows {
            let action: String = row.get("action");
            let status: i64 = row.get("status");
            let meta_raw: String = row.get("meta");
            let meta: Value = serde_json::from_str(&meta_raw).unwrap_or_else(|_| json!({}));
            events.push(EventRow {
                action,
                status,
                meta,
            });
        }
        Ok(events)
    }
}

struct CommandResult {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

struct EventRow {
    action: String,
    status: i64,
    meta: Value,
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

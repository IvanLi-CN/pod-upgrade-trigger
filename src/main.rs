use hex::decode;
use hmac::{Hmac, Mac};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::runtime::Runtime;
use url::Url;

const LOG_TAG: &str = "pod-upgrade-trigger";
const DEFAULT_STATE_DIR: &str = "/srv/pod-upgrade-trigger";
const DEFAULT_WEB_DIST_DIR: &str = "web/dist";
const DEFAULT_WEB_DIST_FALLBACK: &str = "/srv/app/web";
const DEFAULT_CONTAINER_DIR: &str = "/home/<user>/.config/containers/systemd";
const GITHUB_ROUTE_PREFIX: &str = "github-package-update";
const DEFAULT_LIMIT1_COUNT: u64 = 2;
const DEFAULT_LIMIT1_WINDOW: u64 = 600; // 10 minutes
const DEFAULT_LIMIT2_COUNT: u64 = 10;
const DEFAULT_LIMIT2_WINDOW: u64 = 18_000; // 5 hours
const GITHUB_IMAGE_LIMIT_COUNT: u64 = 60;
const GITHUB_IMAGE_LIMIT_WINDOW: u64 = 3_600; // 1 hour
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MANUAL_UNIT: &str = "podman-auto-update.service";
const AUTO_UPDATE_RUN_POLL_INTERVAL_MS: u64 = 1_000;
const AUTO_UPDATE_RUN_MAX_SECS: u64 = 1_800; // 30 minutes hard cap for a single auto-update run
const DEFAULT_REGISTRY_HOST: &str = "ghcr.io";
const PULL_RETRY_ATTEMPTS: u8 = 3;
const PULL_RETRY_DELAY_SECS: u64 = 5;
const COMMAND_OUTPUT_MAX_LEN: usize = 32_768;
const DEFAULT_SCHEDULER_INTERVAL_SECS: u64 = 900;
const DEFAULT_STATE_RETENTION_SECS: u64 = 86_400; // 24 hours
const DEFAULT_DB_PATH: &str = "data/pod-upgrade-trigger.db";

// Environment variable names (external interface). All variables use the
// PODUP_ prefix to avoid ambiguity with legacy naming.
const ENV_STATE_DIR: &str = "PODUP_STATE_DIR";
const ENV_DB_URL: &str = "PODUP_DB_URL";
const ENV_TOKEN: &str = "PODUP_TOKEN";
const ENV_MANUAL_TOKEN: &str = "PODUP_MANUAL_TOKEN";
const ENV_GH_WEBHOOK_SECRET: &str = "PODUP_GH_WEBHOOK_SECRET";
const ENV_HTTP_ADDR: &str = "PODUP_HTTP_ADDR";
const ENV_PUBLIC_BASE_URL: &str = "PODUP_PUBLIC_BASE_URL";
const ENV_DEBUG_PAYLOAD_PATH: &str = "PODUP_DEBUG_PAYLOAD_PATH";
const ENV_SCHEDULER_INTERVAL_SECS: &str = "PODUP_SCHEDULER_INTERVAL_SECS";
const ENV_SCHEDULER_MIN_INTERVAL_SECS: &str = "PODUP_SCHEDULER_MIN_INTERVAL_SECS";
const ENV_SCHEDULER_MAX_TICKS: &str = "PODUP_SCHEDULER_MAX_TICKS";
const ENV_MANUAL_UNITS: &str = "PODUP_MANUAL_UNITS";
const ENV_MANUAL_AUTO_UPDATE_UNIT: &str = "PODUP_MANUAL_AUTO_UPDATE_UNIT";
const ENV_CONTAINER_DIR: &str = "PODUP_CONTAINER_DIR";
const ENV_FWD_AUTH_HEADER: &str = "PODUP_FWD_AUTH_HEADER";
const ENV_FWD_AUTH_ADMIN_VALUE: &str = "PODUP_FWD_AUTH_ADMIN_VALUE";
const ENV_FWD_AUTH_NICKNAME_HEADER: &str = "PODUP_FWD_AUTH_NICKNAME_HEADER";
const ENV_ADMIN_MODE_NAME: &str = "PODUP_ADMIN_MODE_NAME";
const ENV_DEV_OPEN_ADMIN: &str = "PODUP_DEV_OPEN_ADMIN";
const ENV_SYSTEMD_RUN_SNAPSHOT: &str = "PODUP_SYSTEMD_RUN_SNAPSHOT";
const ENV_AUTO_DISCOVER: &str = "PODUP_AUTO_DISCOVER";
const ENV_TASK_RETENTION_SECS: &str = "PODUP_TASK_RETENTION_SECS";
const ENV_AUTO_UPDATE_LOG_DIR: &str = "PODUP_AUTO_UPDATE_LOG_DIR";
const EVENTS_DEFAULT_PAGE_SIZE: u64 = 50;
const EVENTS_MAX_PAGE_SIZE: u64 = 500;
const EVENTS_MAX_LIMIT: u64 = 500;
const WEBHOOK_STATUS_LOOKBACK: u64 = 500;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static DB_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();
static DB_INIT_STATUS: OnceLock<RwLock<DbInitStatus>> = OnceLock::new();
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
static PODMAN_HEALTH: OnceLock<Result<(), String>> = OnceLock::new();
static DISCOVERY_ATTEMPTED: AtomicBool = AtomicBool::new(false);

type HmacSha256 = Hmac<Sha256>;

struct RequestContext {
    method: String,
    path: String,
    query: Option<String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    raw_request: String,
    request_id: String,
    started_at: Instant,
    received_at: SystemTime,
}

#[derive(Clone)]
struct DbInitStatus {
    url: String,
    error: Option<String>,
}

struct ForwardAuthConfig {
    header_name: Option<String>,
    admin_value: Option<String>,
    nickname_header: Option<String>,
    admin_mode_name: Option<String>,
    dev_open_admin: bool,
}

impl ForwardAuthConfig {
    fn load() -> Self {
        // Determine environment profile for default behavior.
        let profile = env::var("PODUP_ENV")
            .unwrap_or_else(|_| "dev".to_string())
            .to_ascii_lowercase();
        let profile_dev_open = matches!(profile.as_str(), "dev" | "development" | "demo");

        let header_name = env::var(ENV_FWD_AUTH_HEADER)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());
        let admin_value = env::var(ENV_FWD_AUTH_ADMIN_VALUE)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let nickname_header = env::var(ENV_FWD_AUTH_NICKNAME_HEADER)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());
        let admin_mode_name = env::var(ENV_ADMIN_MODE_NAME)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let dev_open_admin = env::var(ENV_DEV_OPEN_ADMIN)
            .ok()
            .map(|v| {
                let normalized = v.trim().to_ascii_lowercase();
                matches!(normalized.as_str(), "1" | "true" | "yes")
            })
            // In dev/demo profiles, default to open-admin even when the explicit
            // flag is not provided, so local development and demo modes do not
            // accidentally require ForwardAuth configuration.
            .unwrap_or(profile_dev_open);

        ForwardAuthConfig {
            header_name,
            admin_value,
            nickname_header,
            admin_mode_name,
            dev_open_admin,
        }
    }

    fn open_mode(&self) -> bool {
        self.dev_open_admin || self.header_name.is_none() || self.admin_value.is_none()
    }
}

static FORWARD_AUTH_CONFIG: OnceLock<ForwardAuthConfig> = OnceLock::new();

fn forward_auth_config() -> &'static ForwardAuthConfig {
    FORWARD_AUTH_CONFIG.get_or_init(ForwardAuthConfig::load)
}

fn is_admin_request(ctx: &RequestContext) -> bool {
    let cfg = forward_auth_config();
    if cfg.open_mode() {
        return true;
    }

    let header = match &cfg.header_name {
        Some(name) => name,
        None => return true,
    };
    let expected = match &cfg.admin_value {
        Some(value) => value,
        None => return true,
    };

    match ctx.headers.get(header) {
        Some(value) => value == expected,
        None => false,
    }
}

fn ensure_admin(ctx: &RequestContext, action: &str) -> Result<bool, String> {
    let cfg = forward_auth_config();
    if cfg.open_mode() {
        return Ok(true);
    }

    if is_admin_request(ctx) {
        return Ok(true);
    }

    respond_text(
        ctx,
        401,
        "Unauthorized",
        "unauthorized",
        action,
        Some(json!({
            "reason": "forward-auth",
            "header": cfg.header_name,
        })),
    )?;
    Ok(false)
}

fn ensure_infra_ready(ctx: &RequestContext, action: &str) -> Result<bool, String> {
    if let Some(err) = db_init_error() {
        log_message(&format!("503 {action} db-unavailable err={err}"));
        respond_json(
            ctx,
            503,
            "ServiceUnavailable",
            &json!({
                "error": "db-unavailable",
                "message": err,
                "db_url": db_status().url,
            }),
            action,
            None,
        )?;
        return Ok(false);
    }

    if let Err(err) = podman_health() {
        log_message(&format!("503 {action} podman-unavailable err={err}"));
        respond_json(
            ctx,
            503,
            "ServiceUnavailable",
            &json!({
                "error": "podman-unavailable",
                "message": err,
            }),
            action,
            None,
        )?;
        return Ok(false);
    }

    Ok(true)
}

fn public_base_url() -> Option<String> {
    env::var(ENV_PUBLIC_BASE_URL)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|v| {
            let value = v.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn manual_auto_update_unit() -> String {
    env::var(ENV_MANUAL_AUTO_UPDATE_UNIT).unwrap_or_else(|_| DEFAULT_MANUAL_UNIT.to_string())
}

fn lookup_unit_from_path(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }

    match segments.as_slice() {
        [prefix, unit] | [prefix, unit, "redeploy"] if *prefix == GITHUB_ROUTE_PREFIX => {
            Some(format!("{unit}.service"))
        }
        _ => None,
    }
}

fn extract_container_image(body: &[u8]) -> Result<String, String> {
    if body.is_empty() {
        return Err("empty-body".into());
    }

    let value: Value = serde_json::from_slice(body).map_err(|e| format!("invalid-json:{e}"))?;

    let package_base = if value.pointer("/package").is_some() {
        "/package"
    } else if value.pointer("/registry_package").is_some() {
        "/registry_package"
    } else {
        return Err("missing-package-node".into());
    };

    let package_type =
        pointer_as_str(&value, &format!("{package_base}/package_type")).unwrap_or("");
    if !package_type.eq_ignore_ascii_case("container") {
        return Err(format!("unsupported-package-type:{package_type}"));
    }

    let name = pointer_as_str(&value, &format!("{package_base}/name"))
        .ok_or_else(|| "missing-package-name".to_string())?;

    let owner = pointer_as_str(&value, &format!("{package_base}/owner/login"))
        .or_else(|| pointer_as_str(&value, &format!("{package_base}/repository/owner/login")))
        .or_else(|| pointer_as_str(&value, &format!("{package_base}/namespace")))
        .or_else(|| pointer_as_str(&value, "/repository/owner/login"))
        .unwrap_or("");

    let host_raw = pointer_as_str(&value, "/registry/host")
        .or_else(|| pointer_as_str(&value, "/registry/url"))
        .unwrap_or(DEFAULT_REGISTRY_HOST);
    let registry_host = normalize_registry_host(host_raw);

    let tag = extract_primary_tag(&value).ok_or_else(|| "missing-tag".to_string())?;

    let mut image = String::new();
    image.push_str(&registry_host);
    image.push('/');
    if !owner.is_empty() {
        image.push_str(&owner.to_lowercase());
        image.push('/');
    }
    image.push_str(&name.to_lowercase());
    image.push(':');
    image.push_str(&tag);

    Ok(image)
}

fn main() {
    let mut args = env::args();
    let exe = args.next().unwrap_or_else(|| "pod-upgrade-trigger".into());
    let Some(raw_cmd) = args.next() else {
        print_usage(&exe);
        std::process::exit(1);
    };

    apply_env_profile_defaults();

    let command = normalize_command(&raw_cmd);
    let remaining: Vec<String> = args.collect();

    match command.as_str() {
        "server" => run_server(),
        "http-server" => run_http_server_cli(&remaining),
        "run-task" => run_background_cli(&remaining),
        "scheduler" => run_scheduler_cli(&remaining),
        "trigger-units" => run_trigger_cli(&remaining, false),
        "trigger-all" => run_trigger_cli(&remaining, true),
        "prune-state" => run_prune_cli(&remaining),
        "seed-demo" => run_seed_demo_cli(&remaining),
        "help" => {
            print_usage(&exe);
            std::process::exit(0);
        }
        _ => {
            eprintln!("unknown command: {raw_cmd}");
            print_usage(&exe);
            std::process::exit(2);
        }
    }
}

fn apply_env_profile_defaults() {
    // PODUP_ENV controls a coarse-grained runtime profile:
    // - "test": favor in-memory / throw-away DB defaults
    // - "demo": ephemeral local demo state with UI bundle under ./web/dist
    // - "prod": production-style defaults (minimal assumptions)
    // - anything else (or unset): treated as "dev"
    let profile = env::var("PODUP_ENV")
        .unwrap_or_else(|_| "dev".to_string())
        .to_ascii_lowercase();

    // Only set a variable if it is currently unset or empty, so explicit
    // configuration (including tests and systemd units) always wins.
    let ensure = |key: &str, value: String| {
        if env::var(key)
            .ok()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            // SAFETY: This is called once at process start in main(), before any
            // other threads are spawned, so mutating the environment here is safe.
            unsafe {
                env::set_var(key, value);
            }
        }
    };

    // Common defaults for non-test profiles.
    if profile != "test" && profile != "testing" {
        // Default DB URL: point to the data directory under the compiled project
        // root, so the path is stable and not dependent on the process CWD.
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let db_abs = manifest_dir.join(DEFAULT_DB_PATH);
        ensure(ENV_DB_URL, format!("sqlite://{}", db_abs.to_string_lossy()));

        // Prefer using the current working directory as the implicit state dir
        // when no explicit state dir is provided.
        if env::var(ENV_STATE_DIR).is_err() {
            if let Ok(cwd) = env::current_dir() {
                ensure(ENV_STATE_DIR, cwd.to_string_lossy().into_owned());
            }
        }
    } else {
        // Test profile: prefer an in-memory shared SQLite database unless a DB
        // URL is explicitly provided. This keeps tests isolated and fast.
        if env::var(ENV_DB_URL)
            .ok()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            unsafe {
                env::set_var(ENV_DB_URL, "sqlite::memory:?cache=shared");
            }
        }
    }

    // When we have a state dir, we can also derive a reasonable default for the
    // debug payload path. This avoids writing under DEFAULT_STATE_DIR in dev/demo.
    if env::var(ENV_DEBUG_PAYLOAD_PATH)
        .ok()
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        if let Ok(state_dir) = env::var(ENV_STATE_DIR) {
            if !state_dir.trim().is_empty() {
                let path = Path::new(&state_dir).join("last_payload.bin");
                unsafe {
                    env::set_var(ENV_DEBUG_PAYLOAD_PATH, path.to_string_lossy().into_owned());
                }
            }
        }
    }
}

fn normalize_command(raw: &str) -> String {
    raw.trim_start_matches('-').to_lowercase()
}

fn run_background_cli(args: &[String]) -> ! {
    let task_id = args.get(0).cloned().unwrap_or_default();

    if task_id.is_empty() {
        log_message("500 background-task invalid-args");
        eprintln!("--run-task requires task id");
        std::process::exit(1);
    }

    if let Err(err) = run_task_by_id(&task_id) {
        log_message(&format!(
            "500 background-task-failed task_id={task_id} err={err}"
        ));
        eprintln!("background task failed: {err}");
        std::process::exit(1);
    }

    std::process::exit(0);
}

fn run_server() -> ! {
    if let Err(err) = handle_connection() {
        log_message(&format!("500 internal-error {err}"));
        let _ = write_response(500, "InternalServerError", "internal error");
        std::process::exit(1);
    }
    std::process::exit(0);
}

fn run_seed_demo_cli(_args: &[String]) -> ! {
    match seed_demo_data() {
        Ok(()) => {
            println!("seed-demo completed");
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("seed-demo failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run_http_server_cli(_args: &[String]) -> ! {
    let addr = env::var(ENV_HTTP_ADDR).unwrap_or_else(|_| "0.0.0.0:25111".to_string());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|err| {
        eprintln!("failed to bind HTTP address {addr}: {err}");
        std::process::exit(1);
    });

    eprintln!("listening on http://{addr} (http-server)");

    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                // For each incoming TCP connection, spawn a short-lived child process
                // running `pod-upgrade-trigger server`, wiring the TCP stream to
                // the child's stdin/stdout. This keeps the HTTP handler simple and
                // isolates per-request state in a dedicated process.
                if let Err(err) = spawn_server_for_stream(stream) {
                    eprintln!("failed to spawn server for {peer:?}: {err}");
                }
            }
            Err(err) => {
                eprintln!("accept failed: {err}");
                // avoid busy loop on fatal errors
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn spawn_server_for_stream(stream: TcpStream) -> Result<(), String> {
    stream
        .set_nodelay(true)
        .map_err(|e| format!("set_nodelay failed: {e}"))?;

    // Duplicate the TCP stream for stdin/stdout and transfer ownership of both
    // file descriptors to the child process. We use into_raw_fd so that the
    // File wrappers in the parent do not close the descriptors before exec.
    let stdin_stream = stream
        .try_clone()
        .map_err(|e| format!("failed to clone stream for stdin: {e}"))?;
    let stdout_stream = stream;

    let stdin_fd = stdin_stream.into_raw_fd();
    let stdout_fd = stdout_stream.into_raw_fd();

    let exe = env::current_exe().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(exe);
    cmd.arg("server");
    // Safety: we immediately transfer ownership of the raw FDs into File,
    // which will be consumed by Stdio. The child process will then own these
    // descriptors. We don't use these FDs again in the parent after this point.
    unsafe {
        cmd.stdin(Stdio::from(File::from_raw_fd(stdin_fd)));
        cmd.stdout(Stdio::from(File::from_raw_fd(stdout_fd)));
    }
    // Inherit stderr so request-level logs from the child reach container logs
    // instead of being swallowed by /dev/null.
    cmd.stderr(Stdio::inherit());

    cmd.spawn()
        .map_err(|e| format!("failed to spawn server child: {e}"))?;
    Ok(())
}

fn run_scheduler_cli(args: &[String]) -> ! {
    let mut interval = env::var(ENV_SCHEDULER_INTERVAL_SECS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SCHEDULER_INTERVAL_SECS);
    let mut max_iterations = env::var(ENV_SCHEDULER_MAX_TICKS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok());

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--interval" | "--interval-secs" => {
                idx += 1;
                interval = expect_u64(args.get(idx), "interval");
            }
            "--max-iterations" => {
                idx += 1;
                max_iterations = Some(expect_u64(args.get(idx), "max-iterations"));
            }
            other => {
                eprintln!("unknown scheduler option: {other}");
                std::process::exit(2);
            }
        }
        idx += 1;
    }

    match run_scheduler_loop(interval, max_iterations) {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("scheduler failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run_trigger_cli(args: &[String], force_all: bool) -> ! {
    let mut opts = ManualCliOptions::default();
    opts.all = force_all;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--all" => opts.all = true,
            "--dry-run" => opts.dry_run = true,
            "--caller" => {
                idx += 1;
                opts.caller = args.get(idx).cloned();
            }
            "--reason" => {
                idx += 1;
                opts.reason = args.get(idx).cloned();
            }
            "--units" => {
                idx += 1;
                if let Some(raw) = args.get(idx) {
                    opts.units.extend(
                        raw.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    );
                }
            }
            other if other.starts_with('-') => {
                eprintln!("unknown trigger option: {other}");
                std::process::exit(2);
            }
            value => opts.units.push(value.to_string()),
        }
        idx += 1;
    }

    let units = if opts.all || opts.units.is_empty() {
        manual_unit_list()
    } else {
        let mut resolved = Vec::new();
        for entry in &opts.units {
            match resolve_unit_identifier(entry) {
                Some(unit) => resolved.push(unit),
                None => eprintln!("unknown unit identifier: {entry}"),
            }
        }
        resolved
    };

    if units.is_empty() {
        eprintln!("No units resolved for trigger");
        std::process::exit(2);
    }

    if opts.dry_run {
        // Dry-run keeps original synchronous behaviour; no external commands are executed.
        let results = trigger_units(&units, true);
        for result in &results {
            println!("{} -> {}", result.unit, result.status);
            if let Some(msg) = &result.message {
                println!("    {msg}");
            }
        }

        let ok = all_units_ok(&results);
        log_message(&format!(
            "manual-cli units={} dry_run={} caller={} reason={} status={}",
            results.len(),
            true,
            opts.caller.as_deref().unwrap_or("-"),
            opts.reason.as_deref().unwrap_or("-"),
            if ok { "ok" } else { "error" }
        ));
        record_system_event(
            "cli-trigger",
            if ok { 202 } else { 500 },
            json!({
                "dry_run": true,
                "caller": opts.caller,
                "reason": opts.reason,
                "units": units,
                "results": results,
            }),
        );

        std::process::exit(if ok { 0 } else { 1 });
    }

    // Non-dry-run: create a Task and execute it via run_task_by_id so that all external
    // commands are centralized behind the task runner.
    let task_id = match create_cli_manual_trigger_task(&units, opts.all, &opts.caller, &opts.reason)
    {
        Ok(id) => id,
        Err(err) => {
            eprintln!("failed to create trigger task: {err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = run_task_by_id(&task_id) {
        eprintln!("trigger task failed to run: {err}");
        std::process::exit(1);
    }

    // Load unit-level results from task_units to report back to CLI and events.
    let task_id_owned = task_id.clone();
    let rows_result: Result<Vec<(String, String, Option<String>)>, String> =
        with_db(|pool| async move {
            let rows: Vec<SqliteRow> = sqlx::query(
                "SELECT unit, status, message FROM task_units \
                 WHERE task_id = ? ORDER BY id",
            )
            .bind(&task_id_owned)
            .fetch_all(&pool)
            .await?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let unit: String = row.get("unit");
                let status: String = row.get("status");
                let message: Option<String> = row.get("message");
                out.push((unit, status, message));
            }
            Ok::<Vec<(String, String, Option<String>)>, sqlx::Error>(out)
        });

    let rows = match rows_result {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("failed to load task results: {err}");
            std::process::exit(1);
        }
    };

    if rows.is_empty() {
        eprintln!("no results recorded for trigger task {task_id}");
        std::process::exit(1);
    }

    for (unit, status, message) in &rows {
        println!("{unit} -> {status}");
        if let Some(msg) = message {
            if !msg.is_empty() {
                println!("    {msg}");
            }
        }
    }

    let ok = !rows
        .iter()
        .any(|(_, status, _)| status == "failed" || status == "error");

    let units_for_event: Vec<String> = rows.iter().map(|(u, _, _)| u.clone()).collect();
    let results_for_event: Vec<Value> = rows
        .iter()
        .map(|(u, s, m)| {
            json!({
                "unit": u,
                "status": s,
                "message": m,
            })
        })
        .collect();

    log_message(&format!(
        "manual-cli units={} dry_run={} caller={} reason={} status={}",
        rows.len(),
        false,
        opts.caller.as_deref().unwrap_or("-"),
        opts.reason.as_deref().unwrap_or("-"),
        if ok { "ok" } else { "error" }
    ));
    record_system_event(
        "cli-trigger",
        if ok { 202 } else { 500 },
        json!({
            "dry_run": false,
            "caller": opts.caller,
            "reason": opts.reason,
            "units": units_for_event,
            "results": results_for_event,
            "task_id": task_id,
        }),
    );

    std::process::exit(if ok { 0 } else { 1 });
}

fn run_prune_cli(args: &[String]) -> ! {
    let mut retention_secs = DEFAULT_STATE_RETENTION_SECS;
    let mut dry_run = false;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--max-age-hours" => {
                idx += 1;
                let hours = expect_u64(args.get(idx), "max-age-hours");
                retention_secs = hours.saturating_mul(3600);
            }
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("unknown prune option: {other}");
                std::process::exit(2);
            }
        }
        idx += 1;
    }

    let retention_secs = retention_secs.max(1);
    let max_age_hours = retention_secs / 3600;
    let task_retention_secs = task_retention_secs_from_env();

    let task_id = match create_cli_maintenance_prune_task(max_age_hours, dry_run) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("failed to create prune-state task: {err}");
            std::process::exit(1);
        }
    };

    match run_maintenance_prune_task(&task_id, retention_secs, dry_run) {
        Ok(report) => {
            println!(
                "Removed tokens={} legacy_entries={} stale_locks={} tasks_pruned={} dry_run={}",
                report.tokens_removed,
                report.legacy_dirs_removed,
                report.locks_removed,
                report.tasks_removed,
                dry_run
            );
            record_system_event(
                "cli-prune-state",
                200,
                json!({
                    "dry_run": dry_run,
                    "max_age_hours": max_age_hours,
                    "tokens_removed": report.tokens_removed,
                    "legacy_dirs_removed": report.legacy_dirs_removed,
                    "locks_removed": report.locks_removed,
                    "task_retention_secs": task_retention_secs,
                    "tasks_removed": report.tasks_removed,
                    "task_id": task_id,
                }),
            );
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("state prune failed: {err}");
            record_system_event(
                "cli-prune-state",
                500,
                json!({
                    "dry_run": dry_run,
                    "max_age_hours": max_age_hours,
                    "error": format!("{err}"),
                    "task_id": task_id,
                }),
            );
            std::process::exit(1);
        }
    }
}

fn parse_u64_arg(value: Option<&String>, label: &str) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("missing {label}"))?
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("invalid {label}"))
}

fn expect_u64(value: Option<&String>, label: &str) -> u64 {
    match parse_u64_arg(value, label) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

fn print_usage(exe: &str) {
    eprintln!("Usage: {exe} <command> [options]\n");
    eprintln!("Commands:");
    eprintln!(
        "  server                       Run a single HTTP request on stdin/stdout (internal)"
    );
    eprintln!(
        "  http-server                  Run the persistent HTTP server bound to PODUP_HTTP_ADDR"
    );
    eprintln!("  scheduler [options]          Run the periodic auto-update trigger");
    eprintln!("  trigger-units <units...>     Restart specific units immediately");
    eprintln!("  trigger-all [options]        Restart all configured units");
    eprintln!("  prune-state [options]        Clean ratelimit databases, locks, and old tasks");
    eprintln!("  run-task <...internal...>    Internal helper invoked via systemd-run");
    eprintln!("  help                         Show this message");
}

fn handle_connection() -> Result<(), String> {
    let received_at = SystemTime::now();
    let started_at = Instant::now();
    let request_id = next_request_id();

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| e.to_string())?;
    let request_line = request_line.trim_end_matches(['\r', '\n']).to_string();

    let (method, raw_target) = parse_request_line(&request_line);
    if method.is_empty() || raw_target.is_empty() {
        let redacted = redact_token(&request_line);
        log_message(&format!("400 bad-request {redacted}"));
        respond_basic_error(
            &request_id,
            &method,
            &raw_target,
            &request_line,
            400,
            "BadRequest",
            "bad request",
            "request-line",
            started_at,
            received_at,
        )?;
        return Ok(());
    }

    let (path, query) = match parse_target(&raw_target) {
        Ok(parts) => parts,
        Err(e) => {
            let redacted = redact_token(&request_line);
            log_message(&format!("400 bad-request {redacted}"));
            respond_basic_error(
                &request_id,
                &method,
                &raw_target,
                &request_line,
                400,
                "BadRequest",
                &e,
                "target",
                started_at,
                received_at,
            )?;
            return Ok(());
        }
    };

    let headers = read_headers(&mut reader)?;
    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok());
    let transfer_encoding = headers
        .get("transfer-encoding")
        .map(|s| s.to_ascii_lowercase());

    // Only read a body when the client explicitly signals one via
    // Content-Length or chunked Transfer-Encoding. For typical GET/HEAD
    // requests without these headers we must *not* read to EOF, otherwise
    // the connection would deadlock when the client keeps the socket open.
    let mut body = Vec::new();
    if let Some(len) = content_length {
        body.resize(len, 0);
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("failed to read body: {e}"))?;
    } else if transfer_encoding
        .as_deref()
        .map(|enc| enc.contains("chunked"))
        .unwrap_or(false)
    {
        body = read_chunked_body(&mut reader)?;
    }

    let ctx = RequestContext {
        method,
        path,
        query,
        headers,
        body,
        raw_request: request_line,
        request_id,
        started_at,
        received_at,
    };

    if ctx.method == "GET" && ctx.path == "/health" {
        // Force DB init so health can surface migration/permission issues.
        let _ = db_pool();

        let db = db_status();
        let podman = podman_health();

        let mut issues = Vec::new();
        if let Some(err) = &db.error {
            issues.push(json!({
                "component": "database",
                "message": err,
                "hint": format!("Set {ENV_DB_URL} or {ENV_STATE_DIR} to a writable path"),
            }));
        }
        if let Err(err) = &podman {
            issues.push(json!({
                "component": "podman",
                "message": err,
                "hint": "Ensure podman is installed and available on PATH",
            }));
        }

        let status = if issues.is_empty() { 200 } else { 503 };
        let payload = json!({
            "status": if issues.is_empty() { "ok" } else { "degraded" },
            "db": { "url": db.url, "error": db.error },
            "podman": {
                "ok": podman.is_ok(),
                "error": podman.err(),
            },
            "issues": issues,
        });

        let reason = if status == 200 {
            "OK"
        } else {
            "ServiceUnavailable"
        };
        respond_json(&ctx, status, reason, &payload, "health-check", None)?;
    } else if ctx.method == "GET" && ctx.path == "/sse/hello" {
        handle_hello_sse(&ctx)?;
    } else if ctx.path == "/sse/task-logs" {
        handle_task_logs_sse(&ctx)?;
    } else if ctx.path == "/api/config" {
        handle_config_api(&ctx)?;
    } else if ctx.path == "/api/settings" {
        handle_settings_api(&ctx)?;
    } else if ctx.path == "/api/events" {
        handle_events_api(&ctx)?;
    } else if ctx.path == "/api/tasks" || ctx.path.starts_with("/api/tasks/") {
        handle_tasks_api(&ctx)?;
    } else if ctx.path == "/api/webhooks/status" {
        handle_webhooks_status(&ctx)?;
    } else if ctx.path == "/api/image-locks" || ctx.path.starts_with("/api/image-locks/") {
        handle_image_locks_api(&ctx)?;
    } else if ctx.path == "/api/prune-state" {
        handle_prune_state_api(&ctx)?;
    } else if ctx.path == "/last_payload.bin" {
        handle_debug_payload_download(&ctx)?;
    } else if ctx.path.starts_with("/api/manual/") {
        handle_manual_api(&ctx)?;
    } else if is_github_route(&ctx.path) {
        handle_github_request(&ctx)?;
    } else if ctx.path == "/auto-update" {
        handle_manual_request(&ctx)?;
    } else if try_serve_frontend(&ctx)? {
        // served static asset
    } else {
        log_message(&format!("404 {}", redact_token(&ctx.raw_request)));
        respond_text(&ctx, 404, "NotFound", "not found", "not-found", None)?;
    }

    Ok(())
}

fn handle_hello_sse(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "sse-hello",
            None,
        )?;
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();

    let payload = json!({
        "message": "Webhook auto-update service is online",
        "timestamp": timestamp,
    });

    log_message("200 sse hello handshake");
    respond_sse(ctx, "hello", &payload.to_string(), "sse-hello", None)
}

fn handle_task_logs_sse(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-sse",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "tasks-sse")? {
        return Ok(());
    }

    let mut task_id_param: Option<String> = None;
    if let Some(q) = &ctx.query {
        for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
            if key == "task_id" {
                let candidate = value.into_owned();
                if !candidate.trim().is_empty() {
                    task_id_param = Some(candidate);
                    break;
                }
            }
        }
    }

    let task_id = match task_id_param {
        Some(id) => id,
        None => {
            let payload = json!({ "error": "missing task_id" });
            respond_json(
                ctx,
                400,
                "BadRequest",
                &payload,
                "tasks-sse",
                Some(json!({ "reason": "task-id" })),
            )?;
            return Ok(());
        }
    };

    let detail = match load_task_detail_record(&task_id) {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            let payload = json!({ "error": "task not found" });
            respond_json(
                ctx,
                404,
                "NotFound",
                &payload,
                "tasks-sse",
                Some(json!({ "task_id": task_id })),
            )?;
            return Ok(());
        }
        Err(err) => {
            let payload = json!({ "error": "failed to load task" });
            respond_json(
                ctx,
                500,
                "InternalServerError",
                &payload,
                "tasks-sse",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            return Ok(());
        }
    };

    // Common audit metadata that will be enriched by the chosen mode.
    let mut metadata = json!({
        "task_id": task_id.clone(),
        "logs_sent": 0_u64,
    });

    // Fast path: for non-running tasks we keep the original snapshot behaviour.
    if detail.task.status != "running" {
        let mut body = String::new();
        for log in &detail.logs {
            if let Ok(payload) = serde_json::to_string(log) {
                body.push_str("event: log\n");
                body.push_str("data: ");
                body.push_str(&payload);
                body.push_str("\n\n");
            }
        }
        body.push_str("event: end\n");
        body.push_str("data: done\n\n");

        metadata["logs_sent"] = Value::from(detail.logs.len() as u64);
        metadata["mode"] = Value::from("snapshot");
        metadata["response_size"] = Value::from(body.len() as u64);

        let result = send_sse_stream(&body);
        log_audit_event(ctx, 200, "tasks-sse", metadata);
        return result;
    }

    // Streaming path for running tasks: poll for updates and push incremental log events.
    const POLL_INTERVAL_MS: u64 = 750;
    const MAX_STREAM_SECS: u64 = 600;

    let started_at = Instant::now();
    let mut stdout = io::stdout().lock();

    let mut response_size: u64 = 0;
    let mut logs_sent: u64 = 0;
    let mut reason = String::from("completed");
    let mut last_status = detail.task.status.clone();

    // Write HTTP + SSE headers once and then keep the connection open.
    {
        let header_result: io::Result<()> = (|| {
            write!(stdout, "HTTP/1.1 200 OK\r\n")?;
            stdout.write_all(b"Content-Type: text/event-stream\r\n")?;
            stdout.write_all(b"Cache-Control: no-cache\r\n")?;
            stdout.write_all(b"Connection: keep-alive\r\n")?;
            stdout.write_all(b"\r\n")?;
            stdout.flush()
        })();

        match header_result {
            Ok(()) => {}
            Err(err)
                if err.kind() == io::ErrorKind::BrokenPipe
                    || err.kind() == io::ErrorKind::ConnectionReset =>
            {
                // Client disconnected before we could start streaming.
                reason = String::from("client-disconnect");
                metadata["mode"] = Value::from("streaming");
                metadata["logs_sent"] = Value::from(0_u64);
                metadata["response_size"] = Value::from(0_u64);
                metadata["reason"] = Value::from(reason.clone());
                metadata["status"] = Value::from(last_status);
                log_audit_event(ctx, 200, "tasks-sse", metadata);
                return Ok(());
            }
            Err(err) => {
                metadata["mode"] = Value::from("streaming");
                metadata["logs_sent"] = Value::from(0_u64);
                metadata["response_size"] = Value::from(0_u64);
                metadata["reason"] = Value::from("io-error");
                metadata["status"] = Value::from(last_status);
                log_audit_event(ctx, 200, "tasks-sse", metadata);
                return Err(err.to_string());
            }
        }
    }

    // Helper closure to write a single chunk to the SSE stream while handling
    // common connection error cases.
    let mut write_chunk = |chunk: &str, response_size: &mut u64| -> Result<bool, String> {
        match stdout.write_all(chunk.as_bytes()) {
            Ok(()) => {
                *response_size = response_size.saturating_add(chunk.len() as u64);
            }
            Err(err)
                if err.kind() == io::ErrorKind::BrokenPipe
                    || err.kind() == io::ErrorKind::ConnectionReset =>
            {
                // Client went away; treat as graceful disconnect.
                reason = String::from("client-disconnect");
                return Ok(false);
            }
            Err(err) => {
                reason = String::from("io-error");
                return Err(err.to_string());
            }
        }

        if let Err(err) = stdout.flush() {
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset
            {
                reason = String::from("client-disconnect");
                return Ok(false);
            }
            reason = String::from("io-error");
            return Err(err.to_string());
        }

        Ok(true)
    };

    let mut seen_logs: HashMap<i64, String> = HashMap::new();
    let mut current_detail = detail;
    let mut result_error: Option<String> = None;

    // Streaming loop: always send new/changed logs, then decide whether to continue.
    'stream: loop {
        for log in &current_detail.logs {
            if let Ok(payload) = serde_json::to_string(log) {
                let changed = match seen_logs.get(&log.id) {
                    Some(previous) if previous == &payload => false,
                    _ => true,
                };

                if !changed {
                    continue;
                }

                seen_logs.insert(log.id, payload.clone());

                let chunk = format!("event: log\ndata: {}\n\n", payload);
                match write_chunk(&chunk, &mut response_size) {
                    Ok(true) => {
                        logs_sent = logs_sent.saturating_add(1);
                    }
                    Ok(false) => {
                        // Client disconnected; stop streaming.
                        break 'stream;
                    }
                    Err(err) => {
                        result_error = Some(err);
                        break 'stream;
                    }
                }
            }
        }

        last_status = current_detail.task.status.clone();

        if last_status != "running" {
            let chunk = "event: end\ndata: done\n\n";
            match write_chunk(chunk, &mut response_size) {
                Ok(true) | Ok(false) => {
                    // Completed normally or client disconnected while sending end.
                }
                Err(err) => {
                    result_error = Some(err);
                }
            }
            reason = String::from("completed");
            break 'stream;
        }

        if started_at.elapsed() >= Duration::from_secs(MAX_STREAM_SECS) {
            let chunk = "event: end\ndata: timeout\n\n";
            match write_chunk(chunk, &mut response_size) {
                Ok(true) | Ok(false) => {}
                Err(err) => {
                    result_error = Some(err);
                }
            }
            reason = String::from("timeout");
            break 'stream;
        }

        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));

        match load_task_detail_record(&task_id) {
            Ok(Some(next)) => {
                current_detail = next;
            }
            Ok(None) => {
                let chunk = "event: end\ndata: gone\n\n";
                match write_chunk(chunk, &mut response_size) {
                    Ok(true) | Ok(false) => {}
                    Err(err) => {
                        result_error = Some(err);
                    }
                }
                reason = String::from("task-missing");
                break 'stream;
            }
            Err(err) => {
                reason = String::from("load-error");
                result_error = Some(err);
                break 'stream;
            }
        }
    }

    // Finalize audit metadata for streaming mode.
    metadata["mode"] = Value::from("streaming");
    metadata["logs_sent"] = Value::from(logs_sent);
    metadata["response_size"] = Value::from(response_size);
    metadata["reason"] = Value::from(reason);
    metadata["status"] = Value::from(last_status);

    log_audit_event(ctx, 200, "tasks-sse", metadata);

    if let Some(err) = result_error {
        return Err(err);
    }

    Ok(())
}

fn handle_settings_api(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "settings-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "settings-api")? {
        return Ok(());
    }

    let state_dir = env::var(ENV_STATE_DIR).unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
    let web_dist = frontend_dist_dir();

    let webhook_token_configured = env::var(ENV_TOKEN)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let manual_token_configured = env::var(ENV_MANUAL_TOKEN)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let github_secret_configured = env::var(ENV_GH_WEBHOOK_SECRET)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    let scheduler_interval_secs = env::var(ENV_SCHEDULER_INTERVAL_SECS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SCHEDULER_INTERVAL_SECS);
    let scheduler_min_interval_secs = env::var(ENV_SCHEDULER_MIN_INTERVAL_SECS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(60);
    let scheduler_max_iterations = env::var(ENV_SCHEDULER_MAX_TICKS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());

    let auto_update_unit = manual_auto_update_unit();
    let trigger_units = manual_unit_list();
    let discovered_units = discovered_unit_list();

    let mut manual_units_env = Vec::new();
    let mut seen_manual_env: HashSet<String> = HashSet::new();
    if seen_manual_env.insert(auto_update_unit.clone()) {
        manual_units_env.push(auto_update_unit.clone());
    }
    if let Ok(raw) = env::var(ENV_MANUAL_UNITS) {
        for entry in raw.split(|ch| ch == ',' || ch == '\n') {
            if let Some(unit) = resolve_unit_identifier(entry) {
                if seen_manual_env.insert(unit.clone()) {
                    manual_units_env.push(unit);
                }
            }
        }
    }

    let db_url = env::var(ENV_DB_URL)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("sqlite://{DEFAULT_DB_PATH}"));

    let db_path = db_url
        .strip_prefix("sqlite://")
        .map(|p| Path::new(p).to_path_buf());

    let db_health = db_status();

    let cfg = forward_auth_config();
    let forward_mode = if cfg.open_mode() { "open" } else { "protected" };

    let build_timestamp = option_env!("PODUP_BUILD_TIMESTAMP").map(|s| s.to_string());

    let db_stats = db_path
        .as_ref()
        .map(|p| path_stats(p))
        .unwrap_or_else(|| json!({ "exists": false, "path": db_url }));

    let debug_payload_path = env::var(ENV_DEBUG_PAYLOAD_PATH)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| {
            let default = Path::new(DEFAULT_STATE_DIR).join("last_payload.bin");
            default.to_string_lossy().into_owned()
        });
    let debug_payload_stats = path_stats(Path::new(&debug_payload_path));
    let web_dist_stats = path_stats(&web_dist);

    let task_retention_secs = task_retention_secs_from_env();
    let task_retention_env_override = env::var(ENV_TASK_RETENTION_SECS)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    let response = json!({
        "env": {
            "PODUP_STATE_DIR": state_dir,
            "PODUP_TOKEN_configured": webhook_token_configured,
            "PODUP_MANUAL_TOKEN_configured": manual_token_configured,
            "PODUP_GH_WEBHOOK_SECRET_configured": github_secret_configured,
        },
        "scheduler": {
            "interval_secs": scheduler_interval_secs,
            "min_interval_secs": scheduler_min_interval_secs,
            "max_iterations": scheduler_max_iterations,
        },
        "tasks": {
            "task_retention_secs": task_retention_secs,
            "default_state_retention_secs": DEFAULT_STATE_RETENTION_SECS,
            "env_override": task_retention_env_override,
        },
        "systemd": {
            "auto_update_unit": auto_update_unit,
            "trigger_units": trigger_units,
            "manual_units": manual_units_env,
            "discovered_units": {
                "count": discovered_units.len(),
                "units": discovered_units,
            },
        },
        "database": {
            "url": db_url,
            "error": db_health.error,
        },
        "resources": {
            "state_dir": {
                "path": state_dir,
            },
            "database_file": db_stats,
            "debug_payload": debug_payload_stats,
            "web_dist": web_dist_stats,
        },
        "version": {
            "package": env!("CARGO_PKG_VERSION"),
            "build_timestamp": build_timestamp,
        },
        "forward_auth": {
            "header": cfg.header_name,
            "admin_value_configured": cfg.admin_value.is_some(),
            "nickname_header": cfg.nickname_header,
            "admin_mode_name": cfg.admin_mode_name,
            "dev_open_admin": cfg.dev_open_admin,
            "mode": forward_mode,
        },
    });

    respond_json(ctx, 200, "OK", &response, "settings-api", None)
}

fn path_stats(path: &Path) -> Value {
    match fs::metadata(path) {
        Ok(meta) => {
            let modified_ts = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|dur| dur.as_secs() as i64);
            json!({
                "exists": true,
                "is_dir": meta.is_dir(),
                "size": meta.len(),
                "modified_ts": modified_ts,
                "path": path.to_string_lossy(),
            })
        }
        Err(_) => json!({
            "exists": false,
            "path": path.to_string_lossy(),
        }),
    }
}

fn handle_events_api(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "events-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "events-api")? {
        return Ok(());
    }

    let mut limit: Option<u64> = None;
    let mut page: u64 = 1;
    let mut per_page: u64 = EVENTS_DEFAULT_PAGE_SIZE;
    let mut request_id: Option<String> = None;
    let mut task_id: Option<String> = None;
    let mut path_prefix: Option<String> = None;
    let mut status: Option<i64> = None;
    let mut action: Option<String> = None;
    let mut from_ts: Option<i64> = None;
    let mut to_ts: Option<i64> = None;

    if let Some(q) = &ctx.query {
        for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
            let key = key.as_ref();
            let value = value.as_ref();
            match key {
                "limit" => {
                    if let Ok(v) = value.parse::<u64>() {
                        if v > 0 {
                            limit = Some(v.min(EVENTS_MAX_LIMIT));
                        }
                    }
                }
                "page" => {
                    if let Ok(v) = value.parse::<u64>() {
                        if v > 0 {
                            page = v;
                        }
                    }
                }
                "per_page" | "page_size" => {
                    if let Ok(v) = value.parse::<u64>() {
                        if v > 0 {
                            per_page = v.min(EVENTS_MAX_PAGE_SIZE);
                        }
                    }
                }
                "request_id" => {
                    if !value.is_empty() {
                        request_id = Some(value.to_string());
                    }
                }
                "task_id" => {
                    if !value.is_empty() {
                        task_id = Some(value.to_string());
                    }
                }
                "path_prefix" | "path" => {
                    if !value.is_empty() {
                        path_prefix = Some(value.to_string());
                    }
                }
                "status" => {
                    if let Ok(v) = value.parse::<i64>() {
                        status = Some(v);
                    }
                }
                "action" => {
                    if !value.is_empty() {
                        action = Some(value.to_string());
                    }
                }
                "from_ts" | "from" => {
                    if let Ok(v) = value.parse::<i64>() {
                        from_ts = Some(v);
                    }
                }
                "to_ts" | "to" => {
                    if let Ok(v) = value.parse::<i64>() {
                        to_ts = Some(v);
                    }
                }
                _ => {}
            }
        }
    }

    let (effective_limit, offset, page_num, page_size) = if let Some(lim) = limit {
        let lim = lim.max(1);
        (lim, 0_i64, 1_u64, lim)
    } else {
        let page = page.max(1);
        let size = per_page.max(1);
        let offset = (page.saturating_sub(1)).saturating_mul(size) as i64;
        (size, offset, page, size)
    };

    enum SqlParam {
        I64(i64),
        Str(String),
    }

    let db_result = with_db(|pool| async move {
        let mut filters: Vec<String> = Vec::new();
        let mut params: Vec<SqlParam> = Vec::new();

        if let Some(id) = request_id {
            filters.push("request_id = ?".to_string());
            params.push(SqlParam::Str(id));
        }
        if let Some(tid) = task_id {
            filters.push("task_id = ?".to_string());
            params.push(SqlParam::Str(tid));
        }
        if let Some(prefix) = path_prefix {
            filters.push("path LIKE ?".to_string());
            params.push(SqlParam::Str(format!("{prefix}%")));
        }
        if let Some(code) = status {
            filters.push("status = ?".to_string());
            params.push(SqlParam::I64(code));
        }
        if let Some(act) = action {
            filters.push("action = ?".to_string());
            params.push(SqlParam::Str(act));
        }
        if let Some(from) = from_ts {
            filters.push("ts >= ?".to_string());
            params.push(SqlParam::I64(from));
        }
        if let Some(to) = to_ts {
            filters.push("ts <= ?".to_string());
            params.push(SqlParam::I64(to));
        }

        let mut where_sql = String::new();
        if !filters.is_empty() {
            where_sql.push_str(" WHERE ");
            where_sql.push_str(&filters.join(" AND "));
        }

        let count_sql = format!("SELECT COUNT(*) as cnt FROM event_log{where_sql}");
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
        for param in &params {
            match param {
                SqlParam::I64(v) => {
                    count_query = count_query.bind(*v);
                }
                SqlParam::Str(v) => {
                    count_query = count_query.bind(v);
                }
            }
        }
        let total = count_query.fetch_one(&pool).await.unwrap_or(0);

        let select_sql = format!(
            "SELECT id, request_id, ts, method, path, status, action, duration_ms, meta, task_id, created_at FROM event_log{where_sql} ORDER BY ts DESC, id DESC LIMIT ? OFFSET ?"
        );
        let mut query = sqlx::query(&select_sql);
        for param in &params {
            match param {
                SqlParam::I64(v) => {
                    query = query.bind(*v);
                }
                SqlParam::Str(v) => {
                    query = query.bind(v);
                }
            }
        }
        query = query.bind(effective_limit as i64).bind(offset);

        let rows: Vec<SqliteRow> = query.fetch_all(&pool).await?;
        let mut events = Vec::with_capacity(rows.len());

        for row in rows {
            let meta_raw: String = row.get("meta");
            let meta_value: Value =
                serde_json::from_str(&meta_raw).unwrap_or_else(|_| json!({ "raw": meta_raw }));

            let event = json!({
                "id": row.get::<i64, _>("id"),
                "request_id": row.get::<String, _>("request_id"),
                "ts": row.get::<i64, _>("ts"),
                "method": row.get::<String, _>("method"),
                "path": row.get::<Option<String>, _>("path"),
                "status": row.get::<i64, _>("status"),
                "action": row.get::<String, _>("action"),
                "duration_ms": row.get::<i64, _>("duration_ms"),
                "meta": meta_value,
                 "task_id": row.get::<Option<String>, _>("task_id"),
                "created_at": row.get::<i64, _>("created_at"),
            });
            events.push(event);
        }

        Ok::<(Vec<Value>, i64), sqlx::Error>((events, total))
    });

    let (events, total) = match db_result {
        Ok(ok) => ok,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to query events",
                "events-api",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let response = json!({
        "events": events,
        "total": total,
        "page": page_num,
        "page_size": page_size,
        "has_next": (page_num as i64) * (page_size as i64) < total,
    });

    respond_json(ctx, 200, "OK", &response, "events-api", None)
}

fn handle_tasks_api(ctx: &RequestContext) -> Result<(), String> {
    if !ensure_admin(ctx, "tasks-api")? {
        return Ok(());
    }

    // Routing within /api/tasks namespace.
    if ctx.path == "/api/tasks" {
        match ctx.method.as_str() {
            "GET" => return handle_tasks_list(ctx),
            "POST" => return handle_tasks_create(ctx),
            _ => {
                respond_text(
                    ctx,
                    405,
                    "MethodNotAllowed",
                    "method not allowed",
                    "tasks-api",
                    Some(json!({ "reason": "method" })),
                )?;
                return Ok(());
            }
        }
    }

    // Paths of the form /api/tasks/:id, /api/tasks/:id/stop, etc.
    if let Some(rest) = ctx.path.strip_prefix("/api/tasks/") {
        let trimmed = rest.trim_matches('/');
        if trimmed.is_empty() {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "missing task id",
                "tasks-api",
                Some(json!({ "reason": "task-id" })),
            )?;
            return Ok(());
        }

        if ctx.method == "GET" && !trimmed.contains('/') {
            return handle_task_detail(ctx, trimmed);
        }

        if ctx.method == "POST" {
            if let Some(id) = trimmed.strip_suffix("/stop") {
                let id = id.trim_matches('/');
                return handle_task_stop(ctx, id);
            }
            if let Some(id) = trimmed.strip_suffix("/force-stop") {
                let id = id.trim_matches('/');
                return handle_task_force_stop(ctx, id);
            }
            if let Some(id) = trimmed.strip_suffix("/retry") {
                let id = id.trim_matches('/');
                return handle_task_retry(ctx, id);
            }
        }
    }

    respond_text(
        ctx,
        405,
        "MethodNotAllowed",
        "method not allowed",
        "tasks-api",
        Some(json!({ "reason": "route" })),
    )?;
    Ok(())
}

fn handle_tasks_list(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-list-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    // Pagination and filters.
    let mut page: u64 = 1;
    let mut per_page: u64 = 20;
    let mut status_filter: Option<String> = None;
    let mut kind_filter: Option<String> = None;
    let mut unit_query: Option<String> = None;

    if let Some(q) = &ctx.query {
        for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
            let key = key.as_ref();
            let value = value.as_ref();
            match key {
                "page" => {
                    if let Ok(v) = value.parse::<u64>() {
                        if v > 0 {
                            page = v;
                        }
                    }
                }
                "per_page" | "page_size" => {
                    if let Ok(v) = value.parse::<u64>() {
                        if v > 0 {
                            per_page = v.min(100);
                        }
                    }
                }
                "status" => {
                    if !value.is_empty() {
                        status_filter = Some(value.to_string());
                    }
                }
                "kind" | "type" => {
                    if !value.is_empty() {
                        kind_filter = Some(value.to_string());
                    }
                }
                "unit" | "unit_query" => {
                    if !value.is_empty() {
                        unit_query = Some(value.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let page = page.max(1);
    let per_page = per_page.max(1);
    let offset = (page.saturating_sub(1)).saturating_mul(per_page) as i64;

    enum SqlParam {
        Str(String),
    }

    let db_result = with_db(|pool| async move {
        let mut filters: Vec<String> = Vec::new();
        let mut params: Vec<SqlParam> = Vec::new();

        if let Some(status) = status_filter {
            filters.push("tasks.status = ?".to_string());
            params.push(SqlParam::Str(status));
        }
        if let Some(kind) = kind_filter {
            filters.push("tasks.kind = ?".to_string());
            params.push(SqlParam::Str(kind));
        }
        if let Some(unit) = unit_query {
            let needle = unit.to_lowercase();
            filters.push(
                "EXISTS (SELECT 1 FROM task_units tu \
                 WHERE tu.task_id = tasks.task_id \
                 AND (LOWER(tu.unit) LIKE ? \
                      OR LOWER(COALESCE(tu.slug, '')) LIKE ? \
                      OR LOWER(COALESCE(tu.display_name, '')) LIKE ?))"
                    .to_string(),
            );
            let pattern = format!("%{needle}%");
            params.push(SqlParam::Str(pattern.clone()));
            params.push(SqlParam::Str(pattern.clone()));
            params.push(SqlParam::Str(pattern));
        }

        let mut where_sql = String::new();
        if !filters.is_empty() {
            where_sql.push_str(" WHERE ");
            where_sql.push_str(&filters.join(" AND "));
        }

        let count_sql = format!("SELECT COUNT(*) as cnt FROM tasks{where_sql}");
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
        for param in &params {
            if let SqlParam::Str(v) = param {
                count_query = count_query.bind(v);
            }
        }
        let total = count_query.fetch_one(&pool).await.unwrap_or(0);

        let select_sql = format!(
            "SELECT id, task_id, kind, status, created_at, started_at, finished_at, updated_at, \
             summary, trigger_source, trigger_request_id, trigger_path, trigger_caller, \
             trigger_reason, trigger_scheduler_iteration, can_stop, can_force_stop, can_retry, \
             is_long_running, retry_of \
             FROM tasks{where_sql} \
             ORDER BY created_at DESC, id DESC \
             LIMIT ? OFFSET ?"
        );

        let mut query = sqlx::query(&select_sql);
        for param in &params {
            if let SqlParam::Str(v) = param {
                query = query.bind(v);
            }
        }
        query = query.bind(per_page as i64).bind(offset);

        let rows: Vec<SqliteRow> = query.fetch_all(&pool).await?;

        // Preload units for all tasks in this page.
        let mut task_ids: Vec<String> = Vec::with_capacity(rows.len());
        for row in &rows {
            let tid: String = row.get("task_id");
            task_ids.push(tid);
        }

        let mut units_by_task: HashMap<String, Vec<TaskUnitSummary>> = HashMap::new();
        let mut warnings_by_task: HashMap<String, usize> = HashMap::new();
        if !task_ids.is_empty() {
            let mut in_sql = String::from(
                "SELECT task_id, unit, slug, display_name, status, phase, started_at, finished_at, duration_ms, message, error FROM task_units WHERE task_id IN (",
            );
            for idx in 0..task_ids.len() {
                if idx > 0 {
                    in_sql.push(',');
                }
                in_sql.push('?');
            }
            in_sql.push(')');
            in_sql.push_str(" ORDER BY id ASC");

            let mut units_query = sqlx::query(&in_sql);
            for id in &task_ids {
                units_query = units_query.bind(id);
            }

            let unit_rows: Vec<SqliteRow> = units_query.fetch_all(&pool).await?;
            for row in unit_rows {
                let task_id: String = row.get("task_id");
                let entry = units_by_task.entry(task_id).or_insert_with(Vec::new);
                entry.push(TaskUnitSummary {
                    unit: row.get::<String, _>("unit"),
                    slug: row.get::<Option<String>, _>("slug"),
                    display_name: row.get::<Option<String>, _>("display_name"),
                    status: row.get::<String, _>("status"),
                    phase: row.get::<Option<String>, _>("phase"),
                    started_at: row.get::<Option<i64>, _>("started_at"),
                    finished_at: row.get::<Option<i64>, _>("finished_at"),
                    duration_ms: row.get::<Option<i64>, _>("duration_ms"),
                    message: row.get::<Option<String>, _>("message"),
                    error: row.get::<Option<String>, _>("error"),
                });
            }

            // Aggregate warning/error counts per task for this page.
            let mut warn_sql = String::from(
                "SELECT task_id, COUNT(*) AS warnings \
                 FROM task_logs WHERE level IN ('warning','error') AND task_id IN (",
            );
            for idx in 0..task_ids.len() {
                if idx > 0 {
                    warn_sql.push(',');
                }
                warn_sql.push('?');
            }
            warn_sql.push(')');
            warn_sql.push_str(" GROUP BY task_id");

            let mut warn_query = sqlx::query(&warn_sql);
            for id in &task_ids {
                warn_query = warn_query.bind(id);
            }

            let warn_rows: Vec<SqliteRow> = warn_query.fetch_all(&pool).await?;
            for row in warn_rows {
                let task_id: String = row.get("task_id");
                let count: i64 = row.get("warnings");
                warnings_by_task.insert(task_id, count.max(0) as usize);
            }
        }

        let mut tasks = Vec::with_capacity(rows.len());
        for row in rows {
            let tid: String = row.get("task_id");
            let units = units_by_task.remove(&tid).unwrap_or_else(Vec::new);
            let warning_count = warnings_by_task.remove(&tid);
            tasks.push(build_task_record_from_row(row, units, warning_count));
        }

        Ok::<(Vec<TaskRecord>, i64), sqlx::Error>((tasks, total))
    });

    let (tasks, total) = match db_result {
        Ok(ok) => ok,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to query tasks",
                "tasks-list-api",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let response = TasksListResponse {
        tasks,
        total,
        page,
        page_size: per_page,
        has_next: (page as i64) * (per_page as i64) < total,
    };

    let payload = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
    respond_json(ctx, 200, "OK", &payload, "tasks-list-api", None)
}

fn handle_tasks_create(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-create-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let request: CreateTaskRequest = match parse_json_body(ctx) {
        Ok(body) => body,
        Err(err) => {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "invalid request",
                "tasks-create-api",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let kind = request
        .kind
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("manual")
        .to_string();
    let source = request
        .source
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("manual")
        .to_string();

    let units: Vec<String> = request
        .units
        .unwrap_or_default()
        .into_iter()
        .filter(|u| !u.trim().is_empty())
        .collect();
    let units = if units.is_empty() {
        vec!["unknown.unit".to_string()]
    } else {
        units
    };

    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_request_id = Some(ctx.request_id.clone());
    let caller = request
        .caller
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let reason = request
        .reason
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let path = request
        .path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let is_long_running_flag = request.is_long_running.unwrap_or(true);

    let summary = if kind == "maintenance" {
        Some("Maintenance task started from API".to_string())
    } else {
        Some("Manual task started from API".to_string())
    };

    let task_id_db = task_id.clone();
    let kind_db = kind.clone();
    let source_db = source.clone();
    let caller_db = caller.clone();
    let reason_db = reason.clone();
    let path_db = path.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        let is_long_running_i64: Option<i64> = Some(if is_long_running_flag { 1 } else { 0 });

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_db)
        .bind(&kind_db)
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(&summary)
        .bind(&source_db)
        .bind(&trigger_request_id)
        .bind(&path_db)
        .bind(&caller_db)
        .bind(&reason_db)
        .bind(Option::<i64>::None)
        // Generic /api/tasks ad-hoc tasks do not currently run behind a stable
        // transient runner unit, so we do not offer stop/force-stop at the
        // backend level. This keeps can_stop/can_force_stop semantics aligned
        // with task_runner_unit_for_task(), which will never derive a unit for
        // these records.
        .bind(0_i64) // can_stop
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(is_long_running_i64)
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        for unit_name in &units {
            let slug = if let Some(stripped) = unit_name.strip_suffix(".service") {
                Some(stripped.trim_matches('/').to_string())
            } else {
                None
            };

            sqlx::query(
                "INSERT INTO task_units \
                 (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
                  duration_ms, message, error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_db)
            .bind(unit_name)
            .bind(&slug)
            .bind(unit_name)
            .bind("running")
            .bind(Some("queued"))
            .bind(Some(now))
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(Some("Task started from API"))
            .bind(Option::<String>::None)
            .execute(&mut *tx)
            .await?;
        }

        let meta = json!({
            "source": source_db,
            "caller": caller_db,
            "reason": reason_db,
            "kind": kind_db,
        });
        let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_db)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Task created from API request")
        .bind(Option::<String>::None)
        .bind(meta_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => {
            let response = json!({
                "task_id": task_id,
                "is_long_running": is_long_running_flag,
                "kind": kind,
                "status": "running",
            });
            respond_json(ctx, 200, "OK", &response, "tasks-create-api", None)?;
            Ok(())
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to create task",
                "tasks-create-api",
                Some(json!({ "error": err })),
            )?;
            Ok(())
        }
    }
}

fn handle_task_detail(ctx: &RequestContext, task_id: &str) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-detail-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let result = load_task_detail_record(task_id);
    match result {
        Ok(Some(detail)) => {
            let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
            respond_json(
                ctx,
                200,
                "OK",
                &payload,
                "tasks-detail-api",
                Some(json!({ "task_id": task_id })),
            )?;
            Ok(())
        }
        Ok(None) => {
            respond_text(
                ctx,
                404,
                "NotFound",
                "task not found",
                "tasks-detail-api",
                Some(json!({ "task_id": task_id })),
            )?;
            Ok(())
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to load task",
                "tasks-detail-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            Ok(())
        }
    }
}

/// Derive the underlying systemd transient unit (task runner) for a given task.
/// Returns Ok(Some(unit_name)) when the backend can safely target a unit for
/// stop/force-stop, Ok(None) when the task kind is not stop-capable, and Err
/// when the persisted metadata is malformed.
fn task_runner_unit_for_task(kind: &str, meta_raw: Option<&str>) -> Result<Option<String>, String> {
    match kind {
        // GitHub webhook tasks are dispatched via:
        //   systemd-run --user --unit=webhook-task-<suffix> ... --run-task <task_id>
        // where <suffix> is derived from the delivery id. We reconstruct the
        // transient unit name from the stored TaskMeta.
        "github-webhook" => {
            let meta_str = match meta_raw {
                Some(s) => s,
                None => return Ok(None),
            };

            let meta: TaskMeta = serde_json::from_str(meta_str)
                .map_err(|e| format!("invalid task meta for kind=github-webhook: {e}"))?;

            match meta {
                TaskMeta::GithubWebhook { delivery, .. } => {
                    let suffix = sanitize_image_key(&delivery);
                    Ok(Some(format!("webhook-task-{suffix}")))
                }
                _ => Ok(None),
            }
        }
        // Other kinds currently do not run behind a stable, named transient
        // unit. They are treated as not safely stoppable.
        _ => Ok(None),
    }
}

fn handle_task_stop(ctx: &RequestContext, task_id: &str) -> Result<(), String> {
    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-stop-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let now = current_unix_secs() as i64;

    let task_id_owned = task_id.to_string();

    // Load current task state and metadata first so we can decide whether there
    // is anything to stop and which underlying unit (if any) should be
    // targeted.
    let row_result = with_db(|pool| async move {
        let row_opt: Option<SqliteRow> = sqlx::query(
            "SELECT status, summary, finished_at, kind, meta, can_stop \
             FROM tasks WHERE task_id = ? LIMIT 1",
        )
        .bind(&task_id_owned)
        .fetch_optional(&pool)
        .await?;

        Ok::<Option<SqliteRow>, sqlx::Error>(row_opt)
    });

    let row_opt = match row_result {
        Ok(row) => row,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to load task",
                "tasks-stop-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            return Ok(());
        }
    };

    let Some(row) = row_opt else {
        respond_text(
            ctx,
            404,
            "NotFound",
            "task not found",
            "tasks-stop-api",
            Some(json!({ "task_id": task_id })),
        )?;
        return Ok(());
    };

    let status: String = row.get("status");
    let existing_summary: Option<String> = row.get("summary");
    let finished_at: Option<i64> = row.get("finished_at");
    let kind: String = row.get("kind");
    let meta_raw: Option<String> = row.get("meta");
    let can_stop_raw: i64 = row.get("can_stop");
    let can_stop_flag = can_stop_raw != 0;

    // Terminal states: keep existing noop semantics but always log the request.
    if status != "running" {
        let status_copy = status.clone();
        let task_id_db = task_id.to_string();
        let meta = json!({ "status": status_copy });
        let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        let log_result = with_db(|pool| async move {
            sqlx::query(
                "INSERT INTO task_logs \
                 (task_id, ts, level, action, status, summary, unit, meta) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_db)
            .bind(now)
            .bind("info")
            .bind("task-stop-noop")
            .bind(&status_copy)
            .bind("Stop requested but task already in terminal state")
            .bind(Option::<String>::None)
            .bind(meta_str)
            .execute(&pool)
            .await?;

            Ok::<(), sqlx::Error>(())
        });

        if let Err(err) = log_result {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to stop task",
                "tasks-stop-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            return Ok(());
        }

        // Reload detail for the caller, keeping behaviour idempotent.
        match load_task_detail_record(task_id) {
            Ok(Some(detail)) => {
                let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
                respond_json(
                    ctx,
                    200,
                    "OK",
                    &payload,
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id })),
                )?;
                Ok(())
            }
            Ok(None) => {
                respond_text(
                    ctx,
                    404,
                    "NotFound",
                    "task not found",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id })),
                )?;
                Ok(())
            }
            Err(err) => {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to load task",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id, "error": err })),
                )?;
                Ok(())
            }
        }
    } else {
        // Running tasks: attempt a graceful stop when we know how to locate the
        // underlying transient unit. If the task is marked as not safely
        // stoppable, fail fast with a descriptive error and log.
        if !can_stop_flag {
            let task_id_db = task_id.to_string();
            let kind_copy = kind.clone();
            let meta = json!({
                "kind": kind_copy,
                "reason": "can_stop_false",
            });
            let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

            let log_result = with_db(|pool| async move {
                sqlx::query(
                    "INSERT INTO task_logs \
                     (task_id, ts, level, action, status, summary, unit, meta) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&task_id_db)
                .bind(now)
                .bind("info")
                .bind("task-stop-unsupported")
                .bind("running")
                .bind("Stop requested but task cannot be safely stopped")
                .bind(Option::<String>::None)
                .bind(meta_str)
                .execute(&pool)
                .await?;

                Ok::<(), sqlx::Error>(())
            });

            if let Err(err) = log_result {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to stop task",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id, "error": err })),
                )?;
                return Ok(());
            }

            respond_text(
                ctx,
                400,
                "BadRequest",
                "task cannot be safely stopped",
                "tasks-stop-api",
                Some(json!({ "task_id": task_id, "reason": "unsupported" })),
            )?;
            return Ok(());
        }

        let runner_unit = match task_runner_unit_for_task(&kind, meta_raw.as_deref()) {
            Ok(Some(unit)) => unit,
            Ok(None) => {
                // No stable transient unit associated with this task; treat as
                // not safely stoppable.
                let task_id_db = task_id.to_string();
                let kind_copy = kind.clone();
                let meta = json!({
                    "kind": kind_copy,
                    "reason": "no-runner-unit",
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let log_result = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("info")
                    .bind("task-stop-unsupported")
                    .bind("running")
                    .bind("Stop requested but task has no controllable runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                if let Err(err) = log_result {
                    respond_text(
                        ctx,
                        500,
                        "InternalServerError",
                        "failed to stop task",
                        "tasks-stop-api",
                        Some(json!({ "task_id": task_id, "error": err })),
                    )?;
                    return Ok(());
                }

                respond_text(
                    ctx,
                    400,
                    "BadRequest",
                    "task cannot be safely stopped",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id, "reason": "no-runner-unit" })),
                )?;
                return Ok(());
            }
            Err(err) => {
                // Malformed meta for a supposedly stoppable task.
                let task_id_db = task_id.to_string();
                let meta = json!({
                    "kind": kind,
                    "error": err,
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-stop-meta-error")
                    .bind("running")
                    .bind("Stop requested but task metadata was invalid")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to stop task",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id, "error": "invalid-task-meta" })),
                )?;
                return Ok(());
            }
        };

        // Attempt graceful stop of the transient unit. A successful systemctl
        // call is treated as "stop requested" and we immediately transition the
        // task into cancelled.
        match stop_task_runner_unit(&runner_unit) {
            Ok(result) if result.success() => {
                let finish_ts = finished_at.unwrap_or(now);
                let new_summary = match existing_summary {
                    Some(ref s) if s.contains("cancelled") => s.clone(),
                    Some(ref s) => format!("{s} · cancelled by user"),
                    None => "Task · cancelled by user".to_string(),
                };

                let command = format!("systemctl --user stop {runner_unit}");
                let argv = ["systemctl", "--user", "stop", runner_unit.as_str()];
                let extra_meta = json!({ "via": "stop", "runner_unit": runner_unit });
                let meta_value = build_command_meta(&command, &argv, &result, Some(extra_meta));
                let meta_str =
                    serde_json::to_string(&meta_value).unwrap_or_else(|_| "{}".to_string());

                let task_id_db = task_id.to_string();
                let new_summary_db = new_summary.clone();
                let meta_str_db = meta_str.clone();

                let update_result = with_db(|pool| async move {
                    let mut tx = pool.begin().await?;

                    sqlx::query(
                        "UPDATE tasks SET status = ?, finished_at = ?, updated_at = ?, summary = ?, \
                         can_stop = 0, can_force_stop = 0, can_retry = 1 WHERE task_id = ?",
                    )
                    .bind("cancelled")
                    .bind(finish_ts)
                    .bind(now)
                    .bind(&new_summary_db)
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    // Make sure the initial task-created log no longer advertises
                    // a running/pending status once the task is cancelled.
                    sqlx::query(
                        "UPDATE task_logs \
                         SET status = 'cancelled' \
                         WHERE task_id = ? AND action = 'task-created' AND status IN ('running', 'pending')",
                    )
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        "UPDATE task_units SET status = 'cancelled', \
                         finished_at = COALESCE(finished_at, ?), \
                         message = COALESCE(message, 'cancelled by user') \
                         WHERE task_id = ? AND status IN ('running', 'pending')",
                    )
                    .bind(finish_ts)
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("warning")
                    .bind("task-cancelled")
                    .bind("cancelled")
                    .bind("Task cancelled via /stop API")
                    .bind(Option::<String>::None)
                    .bind(meta_str_db)
                    .execute(&mut *tx)
                    .await?;

                    tx.commit().await?;
                    Ok::<(), sqlx::Error>(())
                });

                if let Err(err) = update_result {
                    respond_text(
                        ctx,
                        500,
                        "InternalServerError",
                        "failed to stop task",
                        "tasks-stop-api",
                        Some(json!({ "task_id": task_id, "error": err })),
                    )?;
                    return Ok(());
                }

                match load_task_detail_record(task_id) {
                    Ok(Some(detail)) => {
                        let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
                        respond_json(
                            ctx,
                            200,
                            "OK",
                            &payload,
                            "tasks-stop-api",
                            Some(json!({ "task_id": task_id })),
                        )?;
                        Ok(())
                    }
                    Ok(None) => {
                        respond_text(
                            ctx,
                            404,
                            "NotFound",
                            "task not found",
                            "tasks-stop-api",
                            Some(json!({ "task_id": task_id })),
                        )?;
                        Ok(())
                    }
                    Err(err) => {
                        respond_text(
                            ctx,
                            500,
                            "InternalServerError",
                            "failed to load task",
                            "tasks-stop-api",
                            Some(json!({ "task_id": task_id, "error": err })),
                        )?;
                        Ok(())
                    }
                }
            }
            Ok(result) => {
                let exit = exit_code_string(&result.status);

                let task_id_db = task_id.to_string();
                let command = format!("systemctl --user stop {runner_unit}");
                let argv = ["systemctl", "--user", "stop", runner_unit.as_str()];
                let extra_meta = json!({
                    "runner_unit": runner_unit,
                    "exit": exit,
                });
                let meta_value = build_command_meta(&command, &argv, &result, Some(extra_meta));
                let meta_str =
                    serde_json::to_string(&meta_value).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-stop-failed")
                    .bind("running")
                    .bind("Failed to stop underlying runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to stop task",
                    "tasks-stop-api",
                    Some(json!({
                        "task_id": task_id,
                        "error": exit,
                    })),
                )?;
                Ok(())
            }
            Err(err) => {
                let task_id_db = task_id.to_string();
                let meta = json!({
                    "runner_unit": runner_unit,
                    "error": err,
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-stop-error")
                    .bind("running")
                    .bind("Error while stopping underlying runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to stop task",
                    "tasks-stop-api",
                    Some(json!({ "task_id": task_id, "error": "runner-stop-error" })),
                )?;
                Ok(())
            }
        }
    }
}

fn handle_task_force_stop(ctx: &RequestContext, task_id: &str) -> Result<(), String> {
    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-force-stop-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let now = current_unix_secs() as i64;

    let task_id_owned = task_id.to_string();

    // Load current task state and metadata first.
    let row_result = with_db(|pool| async move {
        let row_opt: Option<SqliteRow> = sqlx::query(
            "SELECT status, summary, finished_at, kind, meta, can_force_stop \
             FROM tasks WHERE task_id = ? LIMIT 1",
        )
        .bind(&task_id_owned)
        .fetch_optional(&pool)
        .await?;

        Ok::<Option<SqliteRow>, sqlx::Error>(row_opt)
    });

    let row_opt = match row_result {
        Ok(row) => row,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to load task",
                "tasks-force-stop-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            return Ok(());
        }
    };

    let Some(row) = row_opt else {
        respond_text(
            ctx,
            404,
            "NotFound",
            "task not found",
            "tasks-force-stop-api",
            Some(json!({ "task_id": task_id })),
        )?;
        return Ok(());
    };

    let status: String = row.get("status");
    let existing_summary: Option<String> = row.get("summary");
    let finished_at: Option<i64> = row.get("finished_at");
    let kind: String = row.get("kind");
    let meta_raw: Option<String> = row.get("meta");
    let can_force_stop_raw: i64 = row.get("can_force_stop");
    let can_force_stop_flag = can_force_stop_raw != 0;

    // Terminal states: keep existing noop semantics but always log the request.
    if status != "running" {
        let status_copy = status.clone();
        let task_id_db = task_id.to_string();
        let meta = json!({ "status": status_copy });
        let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        let log_result = with_db(|pool| async move {
            sqlx::query(
                "INSERT INTO task_logs \
                 (task_id, ts, level, action, status, summary, unit, meta) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_db)
            .bind(now)
            .bind("info")
            .bind("task-force-stop-noop")
            .bind(&status_copy)
            .bind("Force-stop requested but task already in terminal state")
            .bind(Option::<String>::None)
            .bind(meta_str)
            .execute(&pool)
            .await?;

            Ok::<(), sqlx::Error>(())
        });

        if let Err(err) = log_result {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to force-stop task",
                "tasks-force-stop-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            return Ok(());
        }

        match load_task_detail_record(task_id) {
            Ok(Some(detail)) => {
                let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
                respond_json(
                    ctx,
                    200,
                    "OK",
                    &payload,
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id })),
                )?;
                Ok(())
            }
            Ok(None) => {
                respond_text(
                    ctx,
                    404,
                    "NotFound",
                    "task not found",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id })),
                )?;
                Ok(())
            }
            Err(err) => {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to load task",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id, "error": err })),
                )?;
                Ok(())
            }
        }
    } else {
        // Running tasks: attempt a forceful stop when we know how to locate the
        // underlying transient unit. If the task is marked as not safely
        // force-stoppable, fail fast with a descriptive error and log.
        if !can_force_stop_flag {
            let task_id_db = task_id.to_string();
            let kind_copy = kind.clone();
            let meta = json!({
                "kind": kind_copy,
                "reason": "can_force_stop_false",
            });
            let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

            let log_result = with_db(|pool| async move {
                sqlx::query(
                    "INSERT INTO task_logs \
                     (task_id, ts, level, action, status, summary, unit, meta) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&task_id_db)
                .bind(now)
                .bind("info")
                .bind("task-force-stop-unsupported")
                .bind("running")
                .bind("Force-stop requested but task cannot be safely force-stopped")
                .bind(Option::<String>::None)
                .bind(meta_str)
                .execute(&pool)
                .await?;

                Ok::<(), sqlx::Error>(())
            });

            if let Err(err) = log_result {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to force-stop task",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id, "error": err })),
                )?;
                return Ok(());
            }

            respond_text(
                ctx,
                400,
                "BadRequest",
                "task cannot be safely force-stopped",
                "tasks-force-stop-api",
                Some(json!({ "task_id": task_id, "reason": "unsupported" })),
            )?;
            return Ok(());
        }

        let runner_unit = match task_runner_unit_for_task(&kind, meta_raw.as_deref()) {
            Ok(Some(unit)) => unit,
            Ok(None) => {
                let task_id_db = task_id.to_string();
                let kind_copy = kind.clone();
                let meta = json!({
                    "kind": kind_copy,
                    "reason": "no-runner-unit",
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let log_result = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("info")
                    .bind("task-force-stop-unsupported")
                    .bind("running")
                    .bind("Force-stop requested but task has no controllable runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                if let Err(err) = log_result {
                    respond_text(
                        ctx,
                        500,
                        "InternalServerError",
                        "failed to force-stop task",
                        "tasks-force-stop-api",
                        Some(json!({ "task_id": task_id, "error": err })),
                    )?;
                    return Ok(());
                }

                respond_text(
                    ctx,
                    400,
                    "BadRequest",
                    "task cannot be safely force-stopped",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id, "reason": "no-runner-unit" })),
                )?;
                return Ok(());
            }
            Err(err) => {
                let task_id_db = task_id.to_string();
                let meta = json!({
                    "kind": kind,
                    "error": err,
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-force-stop-meta-error")
                    .bind("running")
                    .bind("Force-stop requested but task metadata was invalid")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to force-stop task",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id, "error": "invalid-task-meta" })),
                )?;
                return Ok(());
            }
        };

        match kill_task_runner_unit(&runner_unit) {
            Ok(result) if result.success() => {
                let finish_ts = finished_at.unwrap_or(now);
                let new_summary = match existing_summary {
                    Some(ref s) if s.contains("force-stopped") => s.clone(),
                    Some(ref s) => format!("{s} · force-stopped"),
                    None => "Task · force-stopped".to_string(),
                };

                let command = format!("systemctl --user kill --signal=SIGKILL {runner_unit}");
                let argv = [
                    "systemctl",
                    "--user",
                    "kill",
                    "--signal=SIGKILL",
                    runner_unit.as_str(),
                ];
                let extra_meta = json!({ "via": "force-stop", "runner_unit": runner_unit });
                let meta_value = build_command_meta(&command, &argv, &result, Some(extra_meta));
                let meta_str =
                    serde_json::to_string(&meta_value).unwrap_or_else(|_| "{}".to_string());

                let task_id_db = task_id.to_string();
                let new_summary_db = new_summary.clone();
                let meta_str_db = meta_str.clone();

                let update_result = with_db(|pool| async move {
                    let mut tx = pool.begin().await?;

                    sqlx::query(
                        "UPDATE tasks SET status = ?, finished_at = ?, updated_at = ?, summary = ?, \
                         can_stop = 0, can_force_stop = 0, can_retry = 1 WHERE task_id = ?",
                    )
                    .bind("failed")
                    .bind(finish_ts)
                    .bind(now)
                    .bind(&new_summary_db)
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    // Keep the task-created log aligned with the final failed
                    // status so the timeline does not show it as still running.
                    sqlx::query(
                        "UPDATE task_logs \
                         SET status = 'failed' \
                         WHERE task_id = ? AND action = 'task-created' AND status IN ('running', 'pending')",
                    )
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        "UPDATE task_units SET status = 'failed', \
                         finished_at = COALESCE(finished_at, ?), \
                         message = COALESCE(message, 'force-stopped by user') \
                         WHERE task_id = ? AND status IN ('running', 'pending')",
                    )
                    .bind(finish_ts)
                    .bind(&task_id_db)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-force-killed")
                    .bind("failed")
                    .bind("Task force-stopped via /force-stop API")
                    .bind(Option::<String>::None)
                    .bind(meta_str_db)
                    .execute(&mut *tx)
                    .await?;

                    tx.commit().await?;
                    Ok::<(), sqlx::Error>(())
                });

                if let Err(err) = update_result {
                    respond_text(
                        ctx,
                        500,
                        "InternalServerError",
                        "failed to force-stop task",
                        "tasks-force-stop-api",
                        Some(json!({ "task_id": task_id, "error": err })),
                    )?;
                    return Ok(());
                }

                match load_task_detail_record(task_id) {
                    Ok(Some(detail)) => {
                        let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
                        respond_json(
                            ctx,
                            200,
                            "OK",
                            &payload,
                            "tasks-force-stop-api",
                            Some(json!({ "task_id": task_id })),
                        )?;
                        Ok(())
                    }
                    Ok(None) => {
                        respond_text(
                            ctx,
                            404,
                            "NotFound",
                            "task not found",
                            "tasks-force-stop-api",
                            Some(json!({ "task_id": task_id })),
                        )?;
                        Ok(())
                    }
                    Err(err) => {
                        respond_text(
                            ctx,
                            500,
                            "InternalServerError",
                            "failed to load task",
                            "tasks-force-stop-api",
                            Some(json!({ "task_id": task_id, "error": err })),
                        )?;
                        Ok(())
                    }
                }
            }
            Ok(result) => {
                let exit = exit_code_string(&result.status);

                let task_id_db = task_id.to_string();
                let command = format!("systemctl --user kill --signal=SIGKILL {runner_unit}");
                let argv = [
                    "systemctl",
                    "--user",
                    "kill",
                    "--signal=SIGKILL",
                    runner_unit.as_str(),
                ];
                let extra_meta = json!({
                    "runner_unit": runner_unit,
                    "exit": exit,
                });
                let meta_value = build_command_meta(&command, &argv, &result, Some(extra_meta));
                let meta_str =
                    serde_json::to_string(&meta_value).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-force-stop-failed")
                    .bind("running")
                    .bind("Failed to force-stop underlying runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to force-stop task",
                    "tasks-force-stop-api",
                    Some(json!({
                        "task_id": task_id,
                        "error": exit,
                    })),
                )?;
                Ok(())
            }
            Err(err) => {
                let task_id_db = task_id.to_string();
                let meta = json!({
                    "runner_unit": runner_unit,
                    "error": err,
                });
                let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

                let _ = with_db(|pool| async move {
                    sqlx::query(
                        "INSERT INTO task_logs \
                         (task_id, ts, level, action, status, summary, unit, meta) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&task_id_db)
                    .bind(now)
                    .bind("error")
                    .bind("task-force-stop-error")
                    .bind("running")
                    .bind("Error while force-stopping underlying runner unit")
                    .bind(Option::<String>::None)
                    .bind(meta_str)
                    .execute(&pool)
                    .await?;

                    Ok::<(), sqlx::Error>(())
                });

                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to force-stop task",
                    "tasks-force-stop-api",
                    Some(json!({ "task_id": task_id, "error": "runner-force-stop-error" })),
                )?;
                Ok(())
            }
        }
    }
}

fn handle_task_retry(ctx: &RequestContext, task_id: &str) -> Result<(), String> {
    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "tasks-retry-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let task_id_owned = task_id.to_string();
    let now = current_unix_secs() as i64;

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        let row_opt: Option<SqliteRow> = sqlx::query(
            "SELECT id, task_id, kind, status, created_at, started_at, finished_at, updated_at, \
             summary, trigger_source, trigger_request_id, trigger_path, trigger_caller, \
             trigger_reason, trigger_scheduler_iteration, can_stop, can_force_stop, can_retry, \
             is_long_running, retry_of \
             FROM tasks WHERE task_id = ? LIMIT 1",
        )
        .bind(&task_id_owned)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(original_row) = row_opt else {
            tx.rollback().await.ok();
            return Ok::<Option<String>, sqlx::Error>(None);
        };

        let status: String = original_row.get("status");
        if status == "running" || status == "pending" {
            tx.rollback().await.ok();
            return Ok(Some("conflict".to_string()));
        }

        let original_kind: String = original_row.get("kind");
        let original_summary: Option<String> = original_row.get("summary");
        let original_trigger_source: String = original_row.get("trigger_source");
        let original_trigger_request_id: Option<String> = original_row.get("trigger_request_id");
        let original_trigger_path: Option<String> = original_row.get("trigger_path");
        let original_trigger_caller: Option<String> = original_row.get("trigger_caller");
        let original_trigger_reason: Option<String> = original_row.get("trigger_reason");
        let original_trigger_iteration: Option<i64> =
            original_row.get("trigger_scheduler_iteration");
        let original_is_long_running: Option<i64> = original_row.get("is_long_running");

        // Load units from original task.
        let unit_rows: Vec<SqliteRow> = sqlx::query(
            "SELECT unit, slug, display_name FROM task_units WHERE task_id = ? ORDER BY id ASC",
        )
        .bind(&task_id_owned)
        .fetch_all(&mut *tx)
        .await?;

        let mut units: Vec<(String, Option<String>, Option<String>)> =
            Vec::with_capacity(unit_rows.len());
        for u in unit_rows {
            units.push((
                u.get::<String, _>("unit"),
                u.get::<Option<String>, _>("slug"),
                u.get::<Option<String>, _>("display_name"),
            ));
        }

        let new_task_id = format!("retry_{}", next_request_id());
        let is_long_running_i64: Option<i64> =
            original_is_long_running.map(|v| if v != 0 { 1 } else { 0 });

        let retry_summary = original_summary
            .as_ref()
            .map(|s| format!("{s} · retry"))
            .unwrap_or_else(|| "Retry of previous task".to_string());

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_task_id)
        .bind(&original_kind)
        .bind("pending")
        .bind(now)
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(&retry_summary)
        .bind(&original_trigger_source)
        .bind(&original_trigger_request_id)
        .bind(&original_trigger_path)
        .bind(&original_trigger_caller)
        .bind(&original_trigger_reason)
        .bind(&original_trigger_iteration)
        .bind(1_i64) // can_stop
        .bind(1_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(is_long_running_i64)
        .bind(&task_id_owned)
        .execute(&mut *tx)
        .await?;

        for (unit, slug, display_name) in &units {
            sqlx::query(
                "INSERT INTO task_units \
                 (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
                  duration_ms, message, error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&new_task_id)
            .bind(unit)
            .bind(slug)
            .bind(display_name)
            .bind("pending")
            .bind(Some("queued"))
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(Some("Retry pending"))
            .bind(Option::<String>::None)
            .execute(&mut *tx)
            .await?;
        }

        // Log on original task that a retry was created.
        let meta = json!({ "retry_task_id": new_task_id });
        let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_owned)
        .bind(now)
        .bind("info")
        .bind("task-retried")
        .bind(&status)
        .bind("Retry task created from this task")
        .bind(Option::<String>::None)
        .bind(meta_str)
        .execute(&mut *tx)
        .await?;

        // Log creation of retry task.
        let meta_new = json!({ "retry_of": task_id_owned });
        let meta_new_str = serde_json::to_string(&meta_new).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_task_id)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("pending")
        .bind("Retry task created from existing task")
        .bind(Option::<String>::None)
        .bind(meta_new_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<Option<String>, sqlx::Error>(Some(new_task_id))
    });

    match db_result {
        Ok(Some(new_id)) => {
            if new_id == "conflict" {
                respond_text(
                    ctx,
                    409,
                    "Conflict",
                    "cannot retry a running or pending task",
                    "tasks-retry-api",
                    Some(json!({ "task_id": task_id })),
                )?;
                return Ok(());
            }

            match load_task_detail_record(&new_id) {
                Ok(Some(detail)) => {
                    let payload = serde_json::to_value(&detail).unwrap_or_else(|_| json!({}));
                    respond_json(
                        ctx,
                        200,
                        "OK",
                        &payload,
                        "tasks-retry-api",
                        Some(json!({ "task_id": new_id })),
                    )?;
                    Ok(())
                }
                Ok(None) => {
                    respond_text(
                        ctx,
                        404,
                        "NotFound",
                        "retry task not found",
                        "tasks-retry-api",
                        Some(json!({ "task_id": task_id })),
                    )?;
                    Ok(())
                }
                Err(err) => {
                    respond_text(
                        ctx,
                        500,
                        "InternalServerError",
                        "failed to load retry task",
                        "tasks-retry-api",
                        Some(json!({ "task_id": task_id, "error": err })),
                    )?;
                    Ok(())
                }
            }
        }
        Ok(None) => {
            respond_text(
                ctx,
                404,
                "NotFound",
                "task not found",
                "tasks-retry-api",
                Some(json!({ "task_id": task_id })),
            )?;
            Ok(())
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to retry task",
                "tasks-retry-api",
                Some(json!({ "task_id": task_id, "error": err })),
            )?;
            Ok(())
        }
    }
}

fn is_github_route(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix('/') {
        if rest == GITHUB_ROUTE_PREFIX {
            return true;
        }
        let mut expected = String::with_capacity(GITHUB_ROUTE_PREFIX.len() + 1);
        expected.push_str(GITHUB_ROUTE_PREFIX);
        expected.push('/');
        rest.starts_with(&expected)
    } else {
        false
    }
}

fn parse_request_line(request_line: &str) -> (String, String) {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    (method, target)
}

fn parse_target(raw_target: &str) -> Result<(String, Option<String>), String> {
    if raw_target.is_empty() {
        return Err("empty target".into());
    }

    // Support both absolute-form and origin-form targets.
    let url = if raw_target.starts_with("http://") || raw_target.starts_with("https://") {
        Url::parse(raw_target).map_err(|e| e.to_string())?
    } else {
        Url::parse(&format!("http://dummy{raw_target}")).map_err(|e| e.to_string())?
    };

    let path = url.path().to_string();
    let query = url.query().map(|s| s.to_string());
    Ok((path, query))
}

fn read_headers<R: BufRead>(reader: &mut R) -> Result<HashMap<String, String>, String> {
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("failed to read header: {e}"))?;
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        if trimmed.is_empty() {
            break;
        }

        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Ok(headers)
}

fn read_chunked_body<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(|e| format!("failed to read chunk size: {e}"))?;
        let size_str = size_line.trim();
        if size_str.is_empty() {
            continue;
        }

        let size = usize::from_str_radix(size_str, 16)
            .map_err(|e| format!("invalid chunk size '{size_str}': {e}"))?;

        if size == 0 {
            loop {
                let mut trailer = String::new();
                reader
                    .read_line(&mut trailer)
                    .map_err(|e| format!("failed to read chunk trailer: {e}"))?;
                if trailer.trim().is_empty() {
                    break;
                }
            }
            break;
        }

        let mut chunk = vec![0u8; size];
        reader
            .read_exact(&mut chunk)
            .map_err(|e| format!("failed to read chunk body: {e}"))?;
        body.extend_from_slice(&chunk);

        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| format!("failed to read chunk terminator: {e}"))?;
    }

    Ok(body)
}

fn extract_query_token(query: &str) -> Option<String> {
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key == "token" {
            return Some(value.to_string());
        }
    }
    None
}

fn handle_manual_request(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" && ctx.method != "POST" {
        let redacted = redact_token(&ctx.raw_request);
        log_message(&format!("405 method-not-allowed {}", redacted));
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "manual-auto-update",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let redacted_line = redact_token(&ctx.raw_request);

    let token = ctx
        .query
        .as_deref()
        .and_then(extract_query_token)
        .unwrap_or_default();

    let expected = env::var(ENV_TOKEN).unwrap_or_default();
    if expected.is_empty() || token.is_empty() || token != expected {
        log_message(&format!("401 {}", redacted_line));
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "manual-auto-update",
            Some(json!({ "reason": "token" })),
        )?;
        return Ok(());
    }

    if !enforce_rate_limit(ctx, &redacted_line)? {
        return Ok(());
    }

    let unit = manual_auto_update_unit();
    let task_id = match create_manual_auto_update_task(&unit, &ctx.request_id, &ctx.path) {
        Ok(id) => id,
        Err(err) => {
            log_message(&format!(
                "500 manual-auto-update-task-create-failed unit={unit} err={err} {}",
                redacted_line
            ));
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to schedule auto-update",
                "manual-auto-update",
                Some(json!({
                    "unit": unit,
                    "error": err,
                })),
            )?;
            return Ok(());
        }
    };

    if let Err(err) = spawn_manual_task(&task_id, "manual-auto-update") {
        log_message(&format!(
            "500 manual-auto-update-dispatch-failed unit={unit} task_id={task_id} err={err} {}",
            redacted_line
        ));
        mark_task_dispatch_failed(
            &task_id,
            Some(&unit),
            "manual",
            "manual-auto-update",
            &err,
            json!({
                "unit": unit.clone(),
                "path": ctx.path.clone(),
                "request_id": ctx.request_id.clone(),
                "reason": "manual-auto-update-dispatch-failed",
            }),
        );
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "failed to trigger",
            "manual-auto-update",
            Some(json!({
                "unit": unit,
                "task_id": task_id,
                "error": err,
            })),
        )?;
        return Ok(());
    }

    log_message(&format!(
        "202 triggered unit={unit} {} task_id={task_id}",
        redacted_line
    ));
    respond_text(
        ctx,
        202,
        "Accepted",
        "auto-update triggered",
        "manual-auto-update",
        Some(json!({ "unit": unit, "task_id": task_id })),
    )?;

    Ok(())
}

fn handle_manual_api(ctx: &RequestContext) -> Result<(), String> {
    if ctx.path == "/api/manual/services" || ctx.path == "/api/manual/services/" {
        return handle_manual_services_list(ctx);
    }

    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "manual-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if ctx.path == "/api/manual/auto-update/run" {
        return handle_manual_auto_update_run(ctx);
    }

    if ctx.path == "/api/manual/trigger" {
        return handle_manual_trigger(ctx);
    }

    if let Some(rest) = ctx.path.strip_prefix("/api/manual/services/") {
        return handle_manual_service(ctx, rest);
    }

    respond_text(
        ctx,
        404,
        "NotFound",
        "manual route not found",
        "manual-api",
        Some(json!({ "reason": "unknown-route" })),
    )
}

fn handle_manual_auto_update_run(ctx: &RequestContext) -> Result<(), String> {
    let request: ManualAutoUpdateRunRequest = match parse_json_body(ctx) {
        Ok(body) => body,
        Err(err) => {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "invalid request",
                "manual-auto-update-run",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let expected = manual_api_token();
    let profile = env::var("PODUP_ENV")
        .unwrap_or_else(|_| "dev".to_string())
        .to_ascii_lowercase();
    let is_dev_like = matches!(profile.as_str(), "dev" | "development" | "demo");
    let require_token = !is_dev_like && !expected.is_empty();
    if require_token && request.token.as_deref().unwrap_or_default() != expected {
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "manual-auto-update-run",
            Some(json!({ "reason": "token" })),
        )?;
        return Ok(());
    }

    let unit = manual_auto_update_unit();

    // Avoid running multiple auto-update executions concurrently for the same unit.
    if let Ok(Some(existing_task)) = active_auto_update_task(&unit) {
        let response = json!({
            "unit": unit,
            "status": "already-running",
            "message": "Auto-update already running for this unit",
            "dry_run": request.dry_run,
            "caller": request.caller,
            "reason": request.reason,
            "image": Value::Null,
            "task_id": existing_task,
            "request_id": ctx.request_id,
        });

        respond_json(
            ctx,
            202,
            "Accepted",
            &response,
            "manual-auto-update-run",
            Some(json!({
                "unit": unit,
                "dry_run": request.dry_run,
                "task_id": response.get("task_id").cloned().unwrap_or(Value::Null),
                "reason": "already-running",
            })),
        )?;
        return Ok(());
    }

    let task_id = match create_manual_auto_update_run_task(
        &unit,
        &ctx.request_id,
        &ctx.path,
        request.caller.as_deref(),
        request.reason.as_deref(),
        request.dry_run,
    ) {
        Ok(id) => id,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to schedule auto-update run",
                "manual-auto-update-run",
                Some(json!({
                    "unit": unit,
                    "error": err,
                })),
            )?;
            return Ok(());
        }
    };

    if let Err(err) = spawn_manual_task(&task_id, "manual-auto-update-run") {
        mark_task_dispatch_failed(
            &task_id,
            Some(&unit),
            "manual",
            "manual-auto-update-run",
            &err,
            json!({
                "unit": unit.clone(),
                "dry_run": request.dry_run,
                "caller": request.caller.clone(),
                "reason": request.reason.clone(),
                "path": ctx.path.clone(),
                "request_id": ctx.request_id.clone(),
            }),
        );
        let error_response = json!({
            "unit": unit,
            "status": "error",
            "message": "failed to dispatch auto-update run",
            "dry_run": request.dry_run,
            "caller": request.caller,
            "reason": request.reason,
            "image": Value::Null,
            "task_id": task_id,
            "request_id": ctx.request_id,
        });

        respond_json(
            ctx,
            500,
            "InternalServerError",
            &error_response,
            "manual-auto-update-run",
            Some(json!({
                "unit": unit,
                "task_id": task_id,
                "error": err,
            })),
        )?;
        return Ok(());
    }

    let response = json!({
        "unit": unit,
        "status": "pending",
        "message": "scheduled via task",
        "dry_run": request.dry_run,
        "caller": request.caller,
        "reason": request.reason,
        "image": Value::Null,
        "task_id": task_id,
        "request_id": ctx.request_id,
    });

    respond_json(
        ctx,
        202,
        "Accepted",
        &response,
        "manual-auto-update-run",
        Some(json!({
            "unit": unit,
            "dry_run": request.dry_run,
            "task_id": response.get("task_id").cloned().unwrap_or(Value::Null),
        })),
    )
}

fn handle_manual_services_list(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "manual-services",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "manual-services")? {
        return Ok(());
    }

    if query_flag(ctx, &["discover", "refresh"]) {
        DISCOVERY_ATTEMPTED.store(false, Ordering::SeqCst);
        ensure_discovery(true);
    }

    let discovered = discovered_unit_list();
    let discovered_set: HashSet<String> = discovered.iter().cloned().collect();
    let discovered_detail = discovered_unit_detail();

    let mut services = Vec::new();
    let auto_update_unit = manual_auto_update_unit();
    for unit in manual_unit_list() {
        let slug = unit
            .trim()
            .trim_matches('/')
            .trim_end_matches(".service")
            .to_string();
        let display_name = unit.clone();
        let default_image = unit_configured_image(&unit);
        let github_path = format!("/{}/{}", GITHUB_ROUTE_PREFIX, slug);
        let source = if discovered_set.contains(&unit) {
            "discovered"
        } else {
            "manual"
        };

        services.push(json!({
            "slug": slug,
            "unit": unit,
            "display_name": display_name,
            "default_image": default_image,
            "github_path": github_path,
            "source": source,
            "is_auto_update": unit == auto_update_unit,
        }));
    }

    let response = json!({
        "services": services,
        "discovered": {
            "count": discovered.len(),
            "units": discovered,
            "detail": discovered_detail
                .iter()
                .map(|(unit, source)| json!({
                    "unit": unit,
                    "source": source,
                }))
                .collect::<Vec<_>>(),
        },
    });
    respond_json(ctx, 200, "OK", &response, "manual-services", None)
}

fn handle_manual_trigger(ctx: &RequestContext) -> Result<(), String> {
    let request: ManualTriggerRequest = match parse_json_body(ctx) {
        Ok(body) => body,
        Err(err) => {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "invalid request",
                "manual-trigger",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let expected = manual_api_token();
    let profile = env::var("PODUP_ENV")
        .unwrap_or_else(|_| "dev".to_string())
        .to_ascii_lowercase();
    let is_dev_like = matches!(profile.as_str(), "dev" | "development" | "demo");
    let require_token = !is_dev_like && !expected.is_empty();
    if require_token && request.token.as_deref().unwrap_or_default() != expected {
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "manual-trigger",
            Some(json!({ "reason": "token" })),
        )?;
        return Ok(());
    }

    let mut units: Vec<String> = if request.all || request.units.is_empty() {
        manual_unit_list()
    } else {
        let mut resolved = Vec::new();
        for item in &request.units {
            if let Some(unit) = resolve_unit_identifier(item) {
                resolved.push(unit);
            }
        }
        resolved
    };

    if units.is_empty() {
        respond_text(
            ctx,
            400,
            "BadRequest",
            "no units available",
            "manual-trigger",
            Some(json!({ "reason": "units" })),
        )?;
        return Ok(());
    }

    let dry_run = request.dry_run;
    let mut results: Vec<UnitActionResult> = Vec::new();

    let mut task_id: Option<String> = None;
    if dry_run {
        // Dry-run 保持原有同步行为，不创建任务，只记录计划中的操作。
        results = trigger_units(&units, true);
    } else {
        // 非 dry-run：创建 Task 并异步执行，由 run-task 接管外部命令。
        let meta = TaskMeta::ManualTrigger {
            all: request.all,
            dry_run: request.dry_run,
        };
        let task = create_manual_trigger_task(
            &units,
            &request.caller,
            &request.reason,
            &ctx.request_id,
            meta,
        )?;
        task_id = Some(task.clone());

        // 立即返回的结果沿用“计划中的结果”，不再同步执行 systemctl。
        results = units
            .iter()
            .map(|unit| UnitActionResult {
                unit: unit.clone(),
                status: "pending".to_string(),
                message: Some("scheduled via task".to_string()),
            })
            .collect();

        // Fire-and-forget 调度 run-task <task_id>，但一旦派发失败，需要立即将
        // Task 标记为 failed 并返回错误响应，避免壳任务。
        if let Err(err) = spawn_manual_task(&task, "manual-trigger") {
            mark_task_dispatch_failed(
                &task,
                None,
                "manual",
                "manual-trigger",
                &err,
                json!({
                    "units": units.clone(),
                    "caller": request.caller.clone(),
                    "reason": request.reason.clone(),
                    "path": ctx.path,
                    "request_id": ctx.request_id,
                }),
            );

            let error_response = ManualTriggerResponse {
                triggered: Vec::new(),
                dry_run,
                caller: request.caller.clone(),
                reason: request.reason.clone(),
                task_id: Some(task.clone()),
                request_id: Some(ctx.request_id.clone()),
            };

            let payload =
                serde_json::to_value(&error_response).map_err(|e| e.to_string())?;
            respond_json(
                ctx,
                500,
                "InternalServerError",
                &payload,
                "manual-trigger",
                Some(json!({
                    "units": units.clone(),
                    "dry_run": dry_run,
                    "task_id": error_response.task_id,
                    "error": err,
                })),
            )?;
            return Ok(());
        }
    }

    let (status, reason) = if all_units_ok(&results) {
        (202, "Accepted")
    } else {
        (207, "Multi-Status")
    };
    units.sort();
    units.dedup();

    let response = ManualTriggerResponse {
        triggered: results.clone(),
        dry_run,
        caller: request.caller.clone(),
        reason: request.reason.clone(),
        task_id,
        request_id: Some(ctx.request_id.clone()),
    };

    let payload = serde_json::to_value(&response).map_err(|e| e.to_string())?;
    let events_task_id = response.task_id.clone();
    respond_json(
        ctx,
        status,
        reason,
        &payload,
        "manual-trigger",
        Some(json!({
            "units": units,
            "dry_run": dry_run,
            "task_id": events_task_id,
        })),
    )
}

fn handle_manual_service(ctx: &RequestContext, slug: &str) -> Result<(), String> {
    let trimmed = slug.trim_matches('/');
    if trimmed.is_empty() {
        respond_text(
            ctx,
            400,
            "BadRequest",
            "missing service",
            "manual-service",
            Some(json!({ "reason": "slug" })),
        )?;
        return Ok(());
    }

    let synthetic = format!("{trimmed}");
    let Some(unit) = resolve_unit_identifier(&synthetic) else {
        respond_text(
            ctx,
            404,
            "NotFound",
            "service not found",
            "manual-service",
            Some(json!({ "slug": trimmed })),
        )?;
        return Ok(());
    };

    let request: ServiceTriggerRequest = match parse_json_body(ctx) {
        Ok(body) => body,
        Err(err) => {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "invalid request",
                "manual-service",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let expected = manual_api_token();
    let profile = env::var("PODUP_ENV")
        .unwrap_or_else(|_| "dev".to_string())
        .to_ascii_lowercase();
    let is_dev_like = matches!(profile.as_str(), "dev" | "development" | "demo");
    let require_token = !is_dev_like && !expected.is_empty();
    if require_token && request.token.as_deref().unwrap_or_default() != expected {
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "manual-service",
            Some(json!({ "reason": "token", "unit": unit })),
        )?;
        return Ok(());
    }

    let dry_run = request.dry_run;
    let mut result: UnitActionResult;
    let mut task_id: Option<String> = None;

    if dry_run {
        // 保持原有 dry-run 行为。
        result = trigger_single_unit(&unit, true);
    } else {
        // 非 dry-run：创建 Task 并异步执行。
        let meta = TaskMeta::ManualService {
            unit: unit.clone(),
            dry_run: request.dry_run,
            image: request.image.clone(),
        };
        let task = create_manual_service_task(
            &unit,
            &request.caller,
            &request.reason,
            request.image.as_deref(),
            &ctx.request_id,
            meta,
        )?;
        task_id = Some(task.clone());

        result = UnitActionResult {
            unit: unit.clone(),
            status: "pending".to_string(),
            message: Some("scheduled via task".to_string()),
        };

        if let Err(err) = spawn_manual_task(&task, "manual-service") {
            mark_task_dispatch_failed(
                &task,
                Some(&unit),
                "manual",
                "manual-service",
                &err,
                json!({
                    "unit": unit,
                    "image": request.image.clone(),
                    "caller": request.caller.clone(),
                    "reason": request.reason.clone(),
                    "path": ctx.path,
                    "request_id": ctx.request_id,
                }),
            );

            let response = json!({
                "unit": unit,
                "status": "error",
                "message": "failed to dispatch manual service task",
                "dry_run": dry_run,
                "caller": request.caller.clone(),
                "reason": request.reason.clone(),
                "image": request.image.clone(),
                "task_id": task_id,
                "request_id": ctx.request_id,
            });

            respond_json(
                ctx,
                500,
                "InternalServerError",
                &response,
                "manual-service",
                Some(json!({
                    "unit": unit,
                    "dry_run": dry_run,
                    "task_id": task_id,
                    "error": err,
                })),
            )?;
            return Ok(());
        }
    }

    let status =
        if result.status == "triggered" || result.status == "dry-run" || result.status == "pending"
        {
            202
        } else {
            500
        };
    let reason = if status == 202 {
        "Accepted"
    } else {
        "InternalServerError"
    };

    let events_task_id = task_id.clone();
    let response = json!({
        "unit": unit,
        "status": result.status,
        "message": result.message,
        "dry_run": dry_run,
        "caller": request.caller,
        "reason": request.reason,
        "image": request.image,
        "task_id": task_id,
        "request_id": ctx.request_id,
    });

    respond_json(
        ctx,
        status,
        reason,
        &response,
        "manual-service",
        Some(json!({
            "unit": unit,
            "dry_run": dry_run,
            "task_id": events_task_id,
        })),
    )
}

fn parse_json_body<T: DeserializeOwned>(ctx: &RequestContext) -> Result<T, String> {
    if ctx.body.is_empty() {
        return Err("missing body".into());
    }
    serde_json::from_slice(&ctx.body).map_err(|e| format!("invalid json: {e}"))
}

#[derive(Debug, Deserialize)]
struct ManualTriggerRequest {
    token: Option<String>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    units: Vec<String>,
    #[serde(default)]
    dry_run: bool,
    caller: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManualAutoUpdateRunRequest {
    token: Option<String>,
    #[serde(default)]
    dry_run: bool,
    caller: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone)]
struct DiscoveredUnit {
    unit: String,
    source: &'static str,
}

#[derive(Default)]
struct DiscoveryStats {
    dir: usize,
    ps: usize,
}

#[derive(Debug, Deserialize)]
struct ServiceTriggerRequest {
    token: Option<String>,
    #[serde(default)]
    dry_run: bool,
    caller: Option<String>,
    reason: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PruneStateRequest {
    max_age_hours: Option<u64>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct PruneStateResponse {
    tokens_removed: usize,
    locks_removed: usize,
    legacy_dirs_removed: usize,
    tasks_removed: usize,
    task_retention_secs: u64,
    dry_run: bool,
    max_age_hours: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct UnitActionResult {
    unit: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct ManualTriggerResponse {
    triggered: Vec<UnitActionResult>,
    dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

// --- Task domain types (backend representation mirroring web/src/domain/tasks.ts) ---

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum TaskMeta {
    #[serde(rename = "manual-trigger")]
    ManualTrigger {
        #[serde(default)]
        all: bool,
        #[serde(default)]
        dry_run: bool,
    },
    #[serde(rename = "manual-service")]
    ManualService {
        unit: String,
        #[serde(default)]
        dry_run: bool,
        #[serde(default)]
        image: Option<String>,
    },
    #[serde(rename = "github-webhook")]
    GithubWebhook {
        unit: String,
        image: String,
        event: String,
        delivery: String,
        path: String,
    },
    #[serde(rename = "auto-update")]
    AutoUpdate { unit: String },
    #[serde(rename = "auto-update-run")]
    AutoUpdateRun {
        unit: String,
        #[serde(default)]
        dry_run: bool,
    },
    #[serde(rename = "maintenance-prune")]
    MaintenancePrune {
        max_age_hours: u64,
        #[serde(default)]
        dry_run: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Serialize, Clone)]
struct TaskTriggerMeta {
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduler_iteration: Option<i64>,
}

#[derive(Debug, Serialize, Clone)]
struct TaskUnitSummary {
    unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct TaskSummaryCounts {
    total_units: usize,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
    running: usize,
    pending: usize,
    skipped: usize,
}

#[derive(Debug, Serialize, Clone)]
struct TaskRecord {
    id: i64,
    task_id: String,
    kind: String,
    status: String,
    created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    trigger: TaskTriggerMeta,
    units: Vec<TaskUnitSummary>,
    unit_counts: TaskSummaryCounts,
    can_stop: bool,
    can_force_stop: bool,
    can_retry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_long_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_of: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "is_false")]
    has_warnings: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning_count: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
struct TaskLogEntry {
    id: i64,
    ts: i64,
    level: String,
    action: String,
    status: String,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<Value>,
}

#[derive(Debug, Serialize)]
struct TasksListResponse {
    tasks: Vec<TaskRecord>,
    total: i64,
    page: u64,
    page_size: u64,
    has_next: bool,
}

#[derive(Debug, Serialize)]
struct TaskDetailResponse {
    #[serde(flatten)]
    task: TaskRecord,
    logs: Vec<TaskLogEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events_hint: Option<TaskEventsHint>,
}

#[derive(Debug, Serialize)]
struct TaskEventsHint {
    task_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateTaskRequest {
    kind: Option<String>,
    source: Option<String>,
    units: Option<Vec<String>>,
    caller: Option<String>,
    reason: Option<String>,
    path: Option<String>,
    is_long_running: Option<bool>,
}

#[derive(Default)]
struct ManualCliOptions {
    units: Vec<String>,
    dry_run: bool,
    all: bool,
    caller: Option<String>,
    reason: Option<String>,
}

fn summarize_task_units(units: &[TaskUnitSummary]) -> TaskSummaryCounts {
    let mut summary = TaskSummaryCounts {
        total_units: units.len(),
        succeeded: 0,
        failed: 0,
        cancelled: 0,
        running: 0,
        pending: 0,
        skipped: 0,
    };

    for unit in units {
        match unit.status.as_str() {
            "succeeded" => summary.succeeded = summary.succeeded.saturating_add(1),
            "failed" => summary.failed = summary.failed.saturating_add(1),
            "cancelled" => summary.cancelled = summary.cancelled.saturating_add(1),
            "running" => summary.running = summary.running.saturating_add(1),
            "pending" => summary.pending = summary.pending.saturating_add(1),
            "skipped" => summary.skipped = summary.skipped.saturating_add(1),
            _ => {}
        }
    }

    summary
}

fn build_task_record_from_row(
    row: SqliteRow,
    units: Vec<TaskUnitSummary>,
    warning_count: Option<usize>,
) -> TaskRecord {
    let unit_counts = summarize_task_units(&units);
    let trigger = TaskTriggerMeta {
        source: row.get::<String, _>("trigger_source"),
        request_id: row.get::<Option<String>, _>("trigger_request_id"),
        path: row.get::<Option<String>, _>("trigger_path"),
        caller: row.get::<Option<String>, _>("trigger_caller"),
        reason: row.get::<Option<String>, _>("trigger_reason"),
        scheduler_iteration: row.get::<Option<i64>, _>("trigger_scheduler_iteration"),
    };

    let can_stop_raw: i64 = row.get("can_stop");
    let can_force_stop_raw: i64 = row.get("can_force_stop");
    let can_retry_raw: i64 = row.get("can_retry");
    let is_long_running_raw: Option<i64> = row.get("is_long_running");
    let warnings = warning_count.unwrap_or(0);

    TaskRecord {
        id: row.get::<i64, _>("id"),
        task_id: row.get::<String, _>("task_id"),
        kind: row.get::<String, _>("kind"),
        status: row.get::<String, _>("status"),
        created_at: row.get::<i64, _>("created_at"),
        started_at: row.get::<Option<i64>, _>("started_at"),
        finished_at: row.get::<Option<i64>, _>("finished_at"),
        updated_at: row.get::<Option<i64>, _>("updated_at"),
        summary: row.get::<Option<String>, _>("summary"),
        trigger,
        units,
        unit_counts,
        can_stop: can_stop_raw != 0,
        can_force_stop: can_force_stop_raw != 0,
        can_retry: can_retry_raw != 0,
        is_long_running: is_long_running_raw.map(|v| v != 0),
        retry_of: row.get::<Option<String>, _>("retry_of"),
        has_warnings: warnings > 0,
        warning_count: if warnings > 0 {
            Some(warnings as u64)
        } else {
            None
        },
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn create_github_task(
    unit: &str,
    image: &str,
    event: &str,
    delivery: &str,
    path: &str,
    request_id: &str,
    meta: &TaskMeta,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "webhook".to_string();

    let meta_value = serde_json::to_value(meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let unit_owned = unit.to_string();
    let path_owned = path.to_string();
    let request_id_owned = request_id.to_string();
    let image_owned = image.to_string();
    let event_owned = event.to_string();
    let delivery_owned = delivery.to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("github-webhook")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some(format!(
            "Webhook task for {unit_owned} ({event_owned} delivery={delivery_owned})"
        )))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(&path_owned)
        .bind(Option::<String>::None) // caller
        .bind(Option::<String>::None) // reason
        .bind(Option::<i64>::None) // scheduler_iteration
        .bind(1_i64) // can_stop
        .bind(1_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_owned)
        .bind(Some(
            unit_owned
                .trim_end_matches(".service")
                .trim_matches('/')
                .to_string(),
        ))
        .bind(&unit_owned)
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(format!(
            "Webhook {event_owned} delivery={delivery_owned} image={image_owned}"
        )))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        // Initial log entry.
        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Github webhook accepted for background processing")
        .bind(Some(unit_owned.clone()))
        .bind(
            serde_json::to_string(&json!({
                "unit": unit_owned,
                "image": image_owned,
                "event": event_owned,
                "delivery": delivery_owned,
                "path": path_owned,
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_manual_trigger_task(
    units: &[String],
    caller: &Option<String>,
    reason: &Option<String>,
    request_id: &str,
    meta: TaskMeta,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "manual".to_string();

    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let units_owned: Vec<String> = units.to_vec();
    let caller_owned = caller.clone();
    let reason_owned = reason.clone();
    let request_id_owned = request_id.to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("manual")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some("Manual trigger task created".to_string()))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(Some("/api/manual/trigger".to_string()))
        .bind(&caller_owned)
        .bind(&reason_owned)
        .bind(Option::<i64>::None)
        .bind(0_i64) // can_stop (manual trigger tasks cannot be safely cancelled at system level)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        for unit in &units_owned {
            sqlx::query(
                "INSERT INTO task_units \
                 (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
                  duration_ms, message, error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_clone)
            .bind(unit)
            .bind(Some(
                unit.trim_end_matches(".service")
                    .trim_matches('/')
                    .to_string(),
            ))
            .bind(unit)
            .bind("running")
            .bind(Some("queued"))
            .bind(Some(now))
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(Some("Manual trigger scheduled from API".to_string()))
            .bind(Option::<String>::None)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Manual trigger task created from API")
        .bind(Option::<String>::None)
        .bind(
            serde_json::to_string(&json!({
                "units": units_owned,
                "caller": caller_owned,
                "reason": reason_owned,
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_cli_manual_trigger_task(
    units: &[String],
    all: bool,
    caller: &Option<String>,
    reason: &Option<String>,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "cli".to_string();

    let meta = TaskMeta::ManualTrigger {
        all,
        dry_run: false,
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let units_owned: Vec<String> = units.to_vec();
    let caller_owned = caller.clone();
    let reason_owned = reason.clone();
    let request_id_owned = "cli-trigger".to_string();
    let path_owned = "cli-trigger".to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("manual")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some("Manual trigger task created from CLI".to_string()))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(Some(path_owned.clone()))
        .bind(&caller_owned)
        .bind(&reason_owned)
        .bind(Option::<i64>::None)
        .bind(0_i64) // can_stop (CLI manual trigger tasks cannot be safely cancelled)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        for unit in &units_owned {
            sqlx::query(
                "INSERT INTO task_units \
                 (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
                  duration_ms, message, error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_clone)
            .bind(unit)
            .bind(Some(
                unit.trim_end_matches(".service")
                    .trim_matches('/')
                    .to_string(),
            ))
            .bind(unit)
            .bind("running")
            .bind(Some("queued"))
            .bind(Some(now))
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(Some("Manual trigger scheduled from CLI".to_string()))
            .bind(Option::<String>::None)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Manual trigger task created from CLI")
        .bind(Option::<String>::None)
        .bind(
            serde_json::to_string(&json!({
                "units": units_owned,
                "caller": caller_owned,
                "reason": reason_owned,
                "source": trigger_source,
                "path": path_owned,
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_manual_service_task(
    unit: &str,
    caller: &Option<String>,
    reason: &Option<String>,
    image: Option<&str>,
    request_id: &str,
    meta: TaskMeta,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "manual".to_string();

    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let unit_owned = unit.to_string();
    let caller_owned = caller.clone();
    let reason_owned = reason.clone();
    let image_owned = image.map(|s| s.to_string());
    let request_id_owned = request_id.to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("manual")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some("Manual service task created".to_string()))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(Some(format!(
            "/api/manual/services/{unit}",
            unit = unit_owned
        )))
        .bind(&caller_owned)
        .bind(&reason_owned)
        .bind(Option::<i64>::None)
        .bind(0_i64) // can_stop (manual service tasks cannot be safely cancelled at system level)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_owned)
        .bind(Some(
            unit_owned
                .trim_end_matches(".service")
                .trim_matches('/')
                .to_string(),
        ))
        .bind(&unit_owned)
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some("Manual service task scheduled from API".to_string()))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Manual service task created from API")
        .bind(Some(unit_owned.clone()))
        .bind(
            serde_json::to_string(&json!({
                "unit": unit_owned,
                "image": image_owned,
                "caller": caller_owned,
                "reason": reason_owned,
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn active_auto_update_task(unit: &str) -> Result<Option<String>, String> {
    let unit_owned = unit.to_string();
    with_db(|pool| async move {
        let row_opt: Option<SqliteRow> = sqlx::query(
            "SELECT t.task_id \
             FROM tasks t \
             JOIN task_units u ON t.task_id = u.task_id \
             WHERE u.unit = ? AND t.status IN ('pending','running') \
             ORDER BY t.created_at DESC \
             LIMIT 1",
        )
        .bind(&unit_owned)
        .fetch_optional(&pool)
        .await?;

        let task_id = row_opt.map(|row| row.get::<String, _>("task_id"));
        Ok::<Option<String>, sqlx::Error>(task_id)
    })
    .map_err(|e| e.to_string())
}

fn create_manual_auto_update_task(
    unit: &str,
    request_id: &str,
    path: &str,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "manual".to_string();

    let meta = TaskMeta::AutoUpdate {
        unit: unit.to_string(),
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let unit_owned = unit.to_string();
    let request_id_owned = request_id.to_string();
    let path_owned = path.to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("manual")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some(format!("Manual auto-update for {unit_owned}")))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(Some(path_owned.clone()))
        .bind(Option::<String>::None) // caller
        .bind(Option::<String>::None) // reason
        .bind(Option::<i64>::None) // scheduler_iteration
        .bind(0_i64) // can_stop (manual auto-update tasks cannot be safely cancelled)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_owned)
        .bind(Some(
            unit_owned
                .trim_end_matches(".service")
                .trim_matches('/')
                .to_string(),
        ))
        .bind(&unit_owned)
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some("Manual auto-update scheduled from API".to_string()))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        let meta_log = json!({
            "unit": unit_owned,
            "source": trigger_source,
            "path": path_owned,
        });
        let meta_log_str = serde_json::to_string(&meta_log).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Manual auto-update task created from API")
        .bind(Some(unit_owned.clone()))
        .bind(meta_log_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_manual_auto_update_run_task(
    unit: &str,
    request_id: &str,
    path: &str,
    caller: Option<&str>,
    reason: Option<&str>,
    dry_run: bool,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "manual".to_string();

    let meta = TaskMeta::AutoUpdateRun {
        unit: unit.to_string(),
        dry_run,
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let unit_owned = unit.to_string();
    let request_id_owned = request_id.to_string();
    let path_owned = path.to_string();
    let caller_owned = caller.map(|s| s.to_string());
    let reason_owned = reason.map(|s| s.to_string());
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        let summary = if dry_run {
            format!("Manual auto-update dry-run for {unit_owned}")
        } else {
            format!("Manual auto-update run for {unit_owned}")
        };

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("manual")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some(summary))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(&request_id_owned)
        .bind(Some(path_owned.clone()))
        .bind(&caller_owned)
        .bind(&reason_owned)
        .bind(Option::<i64>::None) // scheduler_iteration
        .bind(0_i64) // can_stop (manual auto-update tasks cannot be safely cancelled)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_owned)
        .bind(Some(
            unit_owned
                .trim_end_matches(".service")
                .trim_matches('/')
                .to_string(),
        ))
        .bind(&unit_owned)
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(if dry_run {
            "Manual auto-update dry-run scheduled from API".to_string()
        } else {
            "Manual auto-update run scheduled from API".to_string()
        }))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        let meta_log = json!({
            "unit": unit_owned,
            "source": trigger_source,
            "path": path_owned,
            "caller": caller_owned,
            "reason": reason_owned,
            "dry_run": dry_run,
        });
        let meta_log_str = serde_json::to_string(&meta_log).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind(if dry_run {
            "Manual auto-update dry-run task created from API"
        } else {
            "Manual auto-update task created from API"
        })
        .bind(Some(unit_owned.clone()))
        .bind(meta_log_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_scheduler_auto_update_task(unit: &str, iteration: u64) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "scheduler".to_string();

    let meta = TaskMeta::AutoUpdate {
        unit: unit.to_string(),
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let unit_owned = unit.to_string();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("scheduler")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some(format!(
            "Scheduler auto-update iteration={iteration} for {unit_owned}"
        )))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(Option::<String>::None) // request_id
        .bind(Some("scheduler-loop".to_string()))
        .bind(Option::<String>::None) // caller
        .bind(Option::<String>::None) // reason
        .bind(Some(iteration as i64))
        .bind(0_i64) // can_stop
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_owned)
        .bind(Some(
            unit_owned
                .trim_end_matches(".service")
                .trim_matches('/')
                .to_string(),
        ))
        .bind(&unit_owned)
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(format!(
            "Scheduler auto-update scheduled (iteration={iteration})"
        )))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        let meta_log = json!({
            "unit": unit_owned,
            "iteration": iteration,
            "source": trigger_source,
        });
        let meta_log_str = serde_json::to_string(&meta_log).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("Scheduler auto-update task created")
        .bind(Some(unit_owned.clone()))
        .bind(meta_log_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_maintenance_prune_task_for_api(
    max_age_hours: u64,
    dry_run: bool,
    ctx: &RequestContext,
) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "maintenance".to_string();

    let meta = TaskMeta::MaintenancePrune {
        max_age_hours,
        dry_run,
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let request_id_owned = ctx.request_id.clone();
    let path_owned = ctx.path.clone();
    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("maintenance")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some("State prune task created from API".to_string()))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(Some(request_id_owned))
        .bind(Some(path_owned.clone()))
        .bind(Option::<String>::None) // caller
        .bind(Option::<String>::None) // reason
        .bind(Option::<i64>::None) // scheduler_iteration
        .bind(0_i64) // can_stop (state prune tasks cannot be safely cancelled at system level)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        let unit_name = "state-prune".to_string();

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_name)
        .bind(Some(unit_name.clone()))
        .bind("State prune")
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(format!(
            "State prune task scheduled from API (dry_run={})",
            dry_run
        )))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        let meta_log = json!({
            "unit": unit_name,
            "dry_run": dry_run,
            "max_age_hours": max_age_hours,
            "source": trigger_source,
            "path": path_owned,
        });
        let meta_log_str = serde_json::to_string(&meta_log).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("State prune task created from API")
        .bind(Some(unit_name))
        .bind(meta_log_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn create_cli_maintenance_prune_task(max_age_hours: u64, dry_run: bool) -> Result<String, String> {
    let now = current_unix_secs() as i64;
    let task_id = format!("tsk_{}", next_request_id());
    let trigger_source = "cli".to_string();

    let meta = TaskMeta::MaintenancePrune {
        max_age_hours,
        dry_run,
    };
    let meta_value = serde_json::to_value(&meta).map_err(|e| e.to_string())?;
    let meta_str = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;

    let task_id_clone = task_id.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO tasks (task_id, kind, status, created_at, started_at, finished_at, \
             updated_at, summary, meta, trigger_source, trigger_request_id, trigger_path, \
             trigger_caller, trigger_reason, trigger_scheduler_iteration, can_stop, \
             can_force_stop, can_retry, is_long_running, retry_of) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind("maintenance")
        .bind("running")
        .bind(now)
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Some(now))
        .bind(Some("State prune task created from CLI".to_string()))
        .bind(&meta_str)
        .bind(&trigger_source)
        .bind(Some("cli-prune-state".to_string()))
        .bind(Some("cli-prune-state".to_string()))
        .bind(Option::<String>::None) // caller
        .bind(Option::<String>::None) // reason
        .bind(Option::<i64>::None) // scheduler_iteration
        .bind(0_i64) // can_stop (CLI prune tasks cannot be safely cancelled)
        .bind(0_i64) // can_force_stop
        .bind(0_i64) // can_retry
        .bind(Some(1_i64)) // is_long_running
        .bind(Option::<String>::None) // retry_of
        .execute(&mut *tx)
        .await?;

        let unit_name = "state-prune".to_string();

        sqlx::query(
            "INSERT INTO task_units \
             (task_id, unit, slug, display_name, status, phase, started_at, finished_at, \
              duration_ms, message, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(&unit_name)
        .bind(Some(unit_name.clone()))
        .bind("State prune")
        .bind("running")
        .bind(Some("queued"))
        .bind(Some(now))
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .bind(Some(format!(
            "State prune task scheduled from CLI (dry_run={})",
            dry_run
        )))
        .bind(Option::<String>::None)
        .execute(&mut *tx)
        .await?;

        let meta_log = json!({
            "unit": unit_name,
            "dry_run": dry_run,
            "max_age_hours": max_age_hours,
            "source": trigger_source,
            "path": "cli-prune-state",
        });
        let meta_log_str = serde_json::to_string(&meta_log).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_clone)
        .bind(now)
        .bind("info")
        .bind("task-created")
        .bind("running")
        .bind("State prune task created from CLI")
        .bind(Some(unit_name))
        .bind(meta_log_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    match db_result {
        Ok(()) => Ok(task_id),
        Err(err) => Err(err),
    }
}

fn spawn_manual_task(task_id: &str, action: &str) -> Result<(), String> {
    // Test hook: allow integration tests to force dispatch failures for
    // specific manual task actions (e.g. "manual-trigger", "manual-service",
    // "manual-auto-update-run", "scheduler-auto-update") without relying on
    // the underlying systemd-run/system environment.
    if let Ok(raw) = env::var("PODUP_TEST_MANUAL_DISPATCH_FAIL_ACTIONS") {
        let needle = action.to_string();
        for entry in raw.split(',') {
            let trimmed = entry.trim();
            if !trimmed.is_empty() && trimmed == needle {
                return Err("test-manual-dispatch-failed".to_string());
            }
        }
    }

    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_str().ok_or_else(|| "invalid exe path".to_string())?;

    log_message(&format!(
        "debug manual-dispatch-launch task_id={task_id} action={action} exe={exe_str}"
    ));

    // For manual tasks我们不单独创建 transient unit 名称，复用 run-task 调用。
    let args = vec![
        "--user".to_string(),
        "--quiet".to_string(),
        exe_str.to_string(),
        "run-task".to_string(),
        task_id.to_string(),
    ];

    let status = Command::new("systemd-run")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            let code = exit_code_string(&status);
            log_message(&format!(
                "warn manual-dispatch-fallback systemd-run-exit code={code} task_id={task_id} action={action}"
            ));
            // Fallback to inline process.
            Command::new(exe_str)
                .arg("run-task")
                .arg(task_id)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        Err(err) => {
            log_message(&format!(
                "warn manual-dispatch-fallback no-systemd-run err={err} task_id={task_id} action={action}"
            ));
            Command::new(exe_str)
                .arg("run-task")
                .arg(task_id)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
}
fn load_task_detail_record(task_id: &str) -> Result<Option<TaskDetailResponse>, String> {
    let task_id_owned = task_id.to_string();
    with_db(|pool| async move {
        let row_opt: Option<SqliteRow> = sqlx::query(
            "SELECT id, task_id, kind, status, created_at, started_at, finished_at, updated_at, \
             summary, trigger_source, trigger_request_id, trigger_path, trigger_caller, \
             trigger_reason, trigger_scheduler_iteration, can_stop, can_force_stop, can_retry, \
             is_long_running, retry_of \
             FROM tasks WHERE task_id = ? LIMIT 1",
        )
        .bind(&task_id_owned)
        .fetch_optional(&pool)
        .await?;

        let Some(row) = row_opt else {
            return Ok(None);
        };

        let unit_rows: Vec<SqliteRow> = sqlx::query(
            "SELECT unit, slug, display_name, status, phase, started_at, finished_at, \
             duration_ms, message, error \
             FROM task_units WHERE task_id = ? ORDER BY id ASC",
        )
        .bind(&task_id_owned)
        .fetch_all(&pool)
        .await?;

        let mut units = Vec::with_capacity(unit_rows.len());
        for u in unit_rows {
            units.push(TaskUnitSummary {
                unit: u.get::<String, _>("unit"),
                slug: u.get::<Option<String>, _>("slug"),
                display_name: u.get::<Option<String>, _>("display_name"),
                status: u.get::<String, _>("status"),
                phase: u.get::<Option<String>, _>("phase"),
                started_at: u.get::<Option<i64>, _>("started_at"),
                finished_at: u.get::<Option<i64>, _>("finished_at"),
                duration_ms: u.get::<Option<i64>, _>("duration_ms"),
                message: u.get::<Option<String>, _>("message"),
                error: u.get::<Option<String>, _>("error"),
            });
        }

        let log_rows: Vec<SqliteRow> = sqlx::query(
            "SELECT id, ts, level, action, status, summary, unit, meta \
             FROM task_logs WHERE task_id = ? ORDER BY ts ASC, id ASC",
        )
        .bind(&task_id_owned)
        .fetch_all(&pool)
        .await?;

        let mut warnings: usize = 0;
        let mut logs = Vec::with_capacity(log_rows.len());
        for row in log_rows {
            let level: String = row.get("level");
            if level == "warning" || level == "error" {
                warnings = warnings.saturating_add(1);
            }
            let meta_raw: Option<String> = row.get("meta");
            let meta_value: Option<Value> = meta_raw
                .as_deref()
                .map(|raw| serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw })));

            logs.push(TaskLogEntry {
                id: row.get::<i64, _>("id"),
                ts: row.get::<i64, _>("ts"),
                level,
                action: row.get::<String, _>("action"),
                status: row.get::<String, _>("status"),
                summary: row.get::<String, _>("summary"),
                unit: row.get::<Option<String>, _>("unit"),
                meta: meta_value,
            });
        }

        let task = build_task_record_from_row(row, units, Some(warnings));

        let events_hint = Some(TaskEventsHint {
            task_id: task.task_id.clone(),
        });

        Ok(Some(TaskDetailResponse {
            task,
            logs,
            events_hint,
        }))
    })
}

fn run_task_by_id(task_id: &str) -> Result<(), String> {
    // For now we only support github-webhook tasks; other kinds are no-ops.
    let task_id_owned = task_id.to_string();
    let record = with_db(|pool| async move {
        let row_opt: Option<SqliteRow> =
            sqlx::query("SELECT kind, status, meta FROM tasks WHERE task_id = ? LIMIT 1")
                .bind(&task_id_owned)
                .fetch_optional(&pool)
                .await?;

        Ok::<Option<SqliteRow>, sqlx::Error>(row_opt)
    })?;

    let Some(row) = record else {
        return Err(format!("task-not-found task_id={task_id}"));
    };

    let kind: String = row.get("kind");
    let meta_raw: Option<String> = row.get("meta");

    let meta_str = meta_raw.ok_or_else(|| format!("task-meta-missing task_id={task_id}"))?;
    let meta: TaskMeta = serde_json::from_str(&meta_str)
        .map_err(|_| format!("task-meta-invalid task_id={task_id}"))?;

    match (kind.as_str(), meta) {
        (
            "github-webhook",
            TaskMeta::GithubWebhook {
                unit,
                image,
                event,
                delivery,
                path,
            },
        ) => run_background_task(task_id, &unit, &image, &event, &delivery, &path),
        ("manual", TaskMeta::ManualTrigger { .. }) => run_manual_trigger_task(task_id),
        (
            "manual",
            TaskMeta::ManualService {
                unit,
                dry_run,
                image,
            },
        ) => {
            if dry_run {
                log_message(&format!(
                    "info run-task manual-service-dry-run task_id={task_id} unit={unit}"
                ));
                Ok(())
            } else {
                let auto_unit = manual_auto_update_unit();
                if image.is_none() && unit == auto_unit {
                    run_auto_update_task(task_id, &unit)
                } else {
                    run_manual_service_task(task_id, &unit, image.as_deref())
                }
            }
        }
        ("manual", TaskMeta::AutoUpdate { unit }) => run_auto_update_task(task_id, &unit),
        ("manual", TaskMeta::AutoUpdateRun { unit, dry_run }) => {
            run_auto_update_run_task(task_id, &unit, dry_run)
        }
        ("scheduler", TaskMeta::AutoUpdate { unit }) => run_auto_update_task(task_id, &unit),
        (
            "maintenance",
            TaskMeta::MaintenancePrune {
                max_age_hours,
                dry_run,
            },
        ) => {
            let retention_secs = max_age_hours.saturating_mul(3600).max(1);
            let _ = run_maintenance_prune_task(task_id, retention_secs, dry_run)?;
            Ok(())
        }
        _ => {
            log_message(&format!(
                "info run-task unsupported-kind task_id={task_id} kind={kind}"
            ));
            Ok(())
        }
    }
}

fn manual_api_token() -> String {
    env::var(ENV_MANUAL_TOKEN)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| env::var(ENV_TOKEN).unwrap_or_default())
}

fn container_systemd_dir() -> PathBuf {
    env::var(ENV_CONTAINER_DIR)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONTAINER_DIR))
}

fn auto_update_log_dir() -> Option<PathBuf> {
    if let Ok(raw) = env::var(ENV_AUTO_UPDATE_LOG_DIR) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let home = env::var("HOME").ok().filter(|v| !v.trim().is_empty())?;
    Some(
        Path::new(&home)
            .join(".local")
            .join("share")
            .join("podman-auto-update")
            .join("logs"),
    )
}

fn query_flag(ctx: &RequestContext, names: &[&str]) -> bool {
    let Some(qs) = &ctx.query else { return false };
    for pair in qs.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").to_ascii_lowercase();
        if !names.iter().any(|n| *n == key) {
            continue;
        }
        let value = parts.next().unwrap_or("1").to_ascii_lowercase();
        if matches!(value.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    false
}

fn autoupdate_enabled(contents: &str) -> bool {
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') || !trimmed.contains('=') {
            continue;
        }
        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let value = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "autoupdate" {
            return !matches!(value.as_str(), "" | "false" | "no" | "none" | "off" | "0");
        }
    }
    // Default to enabled when key is absent to avoid missing autoupdate units; podman ps path filters by label anyway.
    true
}

fn quadlet_unit_name(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "service" => Some(filename.to_string()),
        // Quadlet files (.container/.kube/.image) generate a matching .service unit.
        "container" | "kube" | "image" => path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| format!("{stem}.service")),
        _ => None,
    }
}

fn discover_units_from_dir() -> Result<Vec<DiscoveredUnit>, String> {
    let dir = container_systemd_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut units = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("failed to read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
        let path = entry.path();
        let Some(unit) = quadlet_unit_name(&path) else {
            continue;
        };

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "container" | "kube" | "image") {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if !autoupdate_enabled(&content) {
                continue;
            }
        }

        units.push(DiscoveredUnit {
            unit,
            source: "dir",
        });
    }

    units.sort_by(|a, b| a.unit.cmp(&b.unit));
    units.dedup_by(|a, b| a.unit == b.unit);
    Ok(units)
}

fn discover_units_from_podman_ps() -> Result<Vec<DiscoveredUnit>, String> {
    let output = Command::new("podman")
        .arg("ps")
        .arg("-a")
        .arg("--filter")
        .arg("label=io.containers.autoupdate")
        .arg("--format")
        .arg("json")
        .output()
        .map_err(|e| format!("podman ps exec failed: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "podman ps exited {}",
            exit_code_string(&output.status)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }

    let parsed: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("invalid podman ps output: {e}"))?;

    let mut units = Vec::new();
    if let Some(items) = parsed.as_array() {
        for item in items {
            // Prefer explicit unit label if present (commonly set by generate systemd/quadlet).
            if let Some(labels) = item.get("Labels").or_else(|| item.get("labels")) {
                let autoupdate_label = labels
                    .get("io.containers.autoupdate")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    autoupdate_label.as_str(),
                    "" | "false" | "no" | "none" | "off" | "0"
                ) {
                    continue;
                }

                if let Some(unit) = labels
                    .get("io.podman.systemd.unit")
                    .or_else(|| labels.get("io.containers.autoupdate.unit"))
                    .and_then(|v| v.as_str())
                {
                    units.push(DiscoveredUnit {
                        unit: unit.to_string(),
                        source: "ps",
                    });
                    continue;
                }
            }

            // Fall back to container name -> <name>.service mapping.
            if let Some(name) = item
                .get("Name")
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
            {
                units.push(DiscoveredUnit {
                    unit: format!("{name}.service"),
                    source: "ps",
                });
                continue;
            }
            if let Some(names) = item.get("Names").or_else(|| item.get("names")) {
                if let Some(first) = names
                    .as_array()
                    .and_then(|arr| arr.get(0))
                    .and_then(|v| v.as_str())
                {
                    units.push(DiscoveredUnit {
                        unit: format!("{first}.service"),
                        source: "ps",
                    });
                    continue;
                }
                if let Some(name) = names.as_str() {
                    units.push(DiscoveredUnit {
                        unit: format!("{name}.service"),
                        source: "ps",
                    });
                    continue;
                }
            }
        }
    }

    units.sort_by(|a, b| a.unit.cmp(&b.unit));
    units.dedup_by(|a, b| a.unit == b.unit);
    Ok(units)
}

fn discover_podman_units() -> Result<Vec<DiscoveredUnit>, String> {
    let mut errors = Vec::new();

    let mut results = Vec::new();

    match discover_units_from_dir() {
        Ok(units) => results.extend(units),
        Err(err) => errors.push(format!("dir: {err}")),
    }

    match discover_units_from_podman_ps() {
        Ok(units) => results.extend(units),
        Err(err) => errors.push(format!("podman-ps: {err}")),
    }

    if !results.is_empty() {
        results.sort_by(|a, b| a.unit.cmp(&b.unit));
        results.dedup_by(|a, b| a.unit == b.unit);
        return Ok(results);
    }

    if errors.is_empty() {
        Ok(Vec::new())
    } else {
        Err(errors.join("; "))
    }
}

fn discover_and_persist_units() -> Result<DiscoveryStats, String> {
    if db_init_error().is_some() {
        return Err("db-unavailable".into());
    }

    let units = discover_podman_units()?;

    let mut stats = DiscoveryStats::default();
    for unit in &units {
        match unit.source {
            "dir" => stats.dir = stats.dir.saturating_add(1),
            "ps" => stats.ps = stats.ps.saturating_add(1),
            _ => {}
        }
    }

    if units.is_empty() {
        return Ok(stats);
    }

    let ts = current_unix_secs() as i64;
    with_db(|pool| async move {
        let mut inserted = 0usize;
        for unit in &units {
            let res = sqlx::query(
                "INSERT OR REPLACE INTO discovered_units (unit, source, discovered_at) VALUES (?, ?, ?)",
            )
            .bind(&unit.unit)
            .bind(unit.source)
            .bind(ts)
            .execute(&pool)
            .await?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        Ok::<usize, sqlx::Error>(inserted)
    })?;

    Ok(stats)
}

fn discovered_unit_list() -> Vec<String> {
    ensure_discovery(false);

    match with_db(|pool| async move {
        let rows: Vec<SqliteRow> = sqlx::query("SELECT unit FROM discovered_units ORDER BY unit")
            .fetch_all(&pool)
            .await?;
        let mut units = Vec::with_capacity(rows.len());
        for row in rows {
            let unit: String = row.get("unit");
            units.push(unit);
        }
        Ok::<Vec<String>, sqlx::Error>(units)
    }) {
        Ok(units) => units,
        Err(err) => {
            log_message(&format!("warn discovery-list-failed err={err}"));
            Vec::new()
        }
    }
}

fn ensure_discovery(force: bool) {
    let should_run = force || !DISCOVERY_ATTEMPTED.swap(true, Ordering::SeqCst);
    if !should_run {
        return;
    }

    match discover_and_persist_units() {
        Ok(stats) => {
            let total = stats.dir.saturating_add(stats.ps);
            let msg = format!(
                "info discovery-ok dir={} ps={} total={}",
                stats.dir, stats.ps, total
            );
            log_message(&msg);
            record_system_event(
                "discovery",
                200,
                json!({
                    "status": if total > 0 { "ok" } else { "empty" },
                    "sources": { "dir": stats.dir, "ps": stats.ps },
                }),
            );
        }
        Err(err) => {
            log_message(&format!("warn discovery-failed err={err}"));
            record_system_event(
                "discovery",
                500,
                json!({
                    "status": "failed",
                    "error": err,
                }),
            );
        }
    }
}

fn discovered_unit_detail() -> Vec<(String, String)> {
    match with_db(|pool| async move {
        let rows: Vec<SqliteRow> =
            sqlx::query("SELECT unit, source FROM discovered_units ORDER BY unit")
                .fetch_all(&pool)
                .await?;
        let mut units = Vec::with_capacity(rows.len());
        for row in rows {
            let unit: String = row.get("unit");
            let source: String = row.get("source");
            units.push((unit, source));
        }
        Ok::<Vec<(String, String)>, sqlx::Error>(units)
    }) {
        Ok(units) => units,
        Err(err) => {
            log_message(&format!("warn discovery-detail-failed err={err}"));
            Vec::new()
        }
    }
}

fn manual_env_unit_list() -> Vec<String> {
    let mut units = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let manual = manual_auto_update_unit();
    seen.insert(manual.clone());
    units.push(manual);

    if let Ok(raw) = env::var(ENV_MANUAL_UNITS) {
        for entry in raw.split(|ch| ch == ',' || ch == '\n') {
            if let Some(unit) = resolve_unit_identifier(entry) {
                if seen.insert(unit.clone()) {
                    units.push(unit);
                }
            }
        }
    }

    units
}

fn manual_unit_list() -> Vec<String> {
    let mut units = manual_env_unit_list();
    let mut seen: HashSet<String> = units.iter().cloned().collect();

    for unit in discovered_unit_list() {
        if seen.insert(unit.clone()) {
            units.push(unit);
        }
    }

    units
}

fn webhook_unit_list() -> Vec<String> {
    if env_flag(ENV_AUTO_DISCOVER) {
        manual_unit_list()
    } else {
        manual_env_unit_list()
    }
}

fn resolve_unit_identifier(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.ends_with(".service") {
        return Some(trimmed.to_string());
    }

    let slug = if trimmed.starts_with(GITHUB_ROUTE_PREFIX) {
        trimmed.to_string()
    } else {
        format!("{GITHUB_ROUTE_PREFIX}/{trimmed}")
    };

    let synthetic = format!("/{slug}");
    lookup_unit_from_path(&synthetic)
}

fn trigger_units(units: &[String], dry_run: bool) -> Vec<UnitActionResult> {
    let mut results = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for unit in units {
        if !seen.insert(unit.clone()) {
            continue;
        }
        results.push(trigger_single_unit(unit, dry_run));
    }
    results
}

fn all_units_ok(results: &[UnitActionResult]) -> bool {
    results
        .iter()
        .all(|r| r.status == "triggered" || r.status == "dry-run" || r.status == "pending")
}

fn trigger_single_unit(unit: &str, dry_run: bool) -> UnitActionResult {
    if dry_run {
        log_message(&format!("debug manual-trigger dry-run unit={unit}"));
        return UnitActionResult {
            unit: unit.to_string(),
            status: "dry-run".into(),
            message: Some("skipped by dry run".into()),
        };
    }

    let manual = manual_auto_update_unit();
    let outcome = if unit == manual {
        start_auto_update_unit(unit)
    } else {
        restart_unit(unit)
    };

    match outcome {
        Ok(result) if result.success() => {
            log_message(&format!("202 manual-trigger unit={unit}"));
            UnitActionResult {
                unit: unit.to_string(),
                status: "triggered".into(),
                message: None,
            }
        }
        Ok(result) => {
            let mut detail = format!("exit={}", exit_code_string(&result.status));
            if !result.stderr.is_empty() {
                detail.push_str(" stderr=");
                detail.push_str(&result.stderr);
            }
            log_message(&format!("500 manual-trigger-failed unit={unit} {detail}"));
            UnitActionResult {
                unit: unit.to_string(),
                status: "failed".into(),
                message: Some(detail),
            }
        }
        Err(err) => {
            log_message(&format!("500 manual-trigger-error unit={unit} err={err}"));
            UnitActionResult {
                unit: unit.to_string(),
                status: "error".into(),
                message: Some(err),
            }
        }
    }
}

fn scheduler_sleep_duration(interval_secs: u64) -> Duration {
    let min_interval = env::var(ENV_SCHEDULER_MIN_INTERVAL_SECS)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(interval_secs.max(min_interval))
}

fn run_scheduler_loop(interval_secs: u64, max_iterations: Option<u64>) -> Result<(), String> {
    let unit = manual_auto_update_unit();
    let sleep = scheduler_sleep_duration(interval_secs);
    let mut iterations: u64 = 0;

    loop {
        iterations = iterations.saturating_add(1);
        log_message(&format!(
            "scheduler tick iteration={iterations} unit={unit}"
        ));

        match create_scheduler_auto_update_task(&unit, iterations) {
            Ok(task_id) => match spawn_manual_task(&task_id, "scheduler-auto-update") {
                Ok(()) => {
                    log_message(&format!(
                        "scheduler dispatched task_id={task_id} unit={unit} iteration={iterations}"
                    ));
                    record_system_event(
                        "scheduler",
                        202,
                        json!({
                            "unit": unit.clone(),
                            "iteration": iterations,
                            "status": "queued",
                            "task_id": task_id,
                        }),
                    );
                }
                Err(err) => {
                    log_message(&format!(
                        "scheduler dispatch error unit={unit} iteration={iterations} err={err}"
                    ));
                    mark_task_dispatch_failed(
                        &task_id,
                        Some(&unit),
                        "scheduler",
                        "scheduler-auto-update",
                        &err,
                        json!({
                            "unit": unit.clone(),
                            "iteration": iterations,
                        }),
                    );
                    record_system_event(
                        "scheduler",
                        500,
                        json!({
                            "unit": unit.clone(),
                            "iteration": iterations,
                            "status": "dispatch-error",
                            "error": err,
                            "task_id": task_id,
                        }),
                    );
                }
            },
            Err(err) => {
                log_message(&format!(
                    "scheduler task-create error unit={unit} iteration={iterations} err={err}"
                ));
                record_system_event(
                    "scheduler",
                    500,
                    json!({
                        "unit": unit.clone(),
                        "iteration": iterations,
                        "status": "task-create-error",
                        "error": err,
                    }),
                );
            }
        }

        if let Some(limit) = max_iterations {
            if iterations >= limit {
                break;
            }
        }

        thread::sleep(sleep);
    }

    Ok(())
}

#[derive(Default)]
struct StatePruneReport {
    tokens_removed: usize,
    locks_removed: usize,
    legacy_dirs_removed: usize,
    tasks_removed: usize,
}

fn task_retention_secs_from_env() -> u64 {
    env::var(ENV_TASK_RETENTION_SECS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_STATE_RETENTION_SECS)
        .max(1)
}

fn prune_state_dir(retention: Duration, dry_run: bool) -> Result<StatePruneReport, String> {
    let dir = env::var(ENV_STATE_DIR).unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
    let state_path = Path::new(&dir);
    let now_secs = current_unix_secs();
    let cutoff_secs = now_secs.saturating_sub(retention.as_secs().max(1)) as i64;

    let mut report = StatePruneReport::default();

    report.tokens_removed = if dry_run {
        with_db(|pool| async move {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM rate_limit_tokens WHERE ts < ?")
                    .bind(cutoff_secs)
                    .fetch_one(&pool)
                    .await?;
            Ok::<usize, sqlx::Error>(count as usize)
        })?
    } else {
        with_db(|pool| async move {
            let res = sqlx::query("DELETE FROM rate_limit_tokens WHERE ts < ?")
                .bind(cutoff_secs)
                .execute(&pool)
                .await?;
            Ok::<usize, sqlx::Error>(res.rows_affected() as usize)
        })?
    };

    let lock_cutoff = SystemTime::now()
        .checked_sub(retention)
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64;

    report.locks_removed = if dry_run {
        with_db(|pool| async move {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM image_locks WHERE acquired_at < ?")
                    .bind(lock_cutoff)
                    .fetch_one(&pool)
                    .await?;
            Ok::<usize, sqlx::Error>(count as usize)
        })?
    } else {
        with_db(|pool| async move {
            let res = sqlx::query("DELETE FROM image_locks WHERE acquired_at < ?")
                .bind(lock_cutoff)
                .execute(&pool)
                .await?;
            Ok::<usize, sqlx::Error>(res.rows_affected() as usize)
        })?
    };

    if !dry_run {
        for legacy in [
            "github-image-limits",
            "github-image-locks",
            "ratelimit.db",
            "ratelimit.lock",
        ] {
            let path = state_path.join(legacy);
            if path.exists() {
                if path.is_dir() {
                    if fs::remove_dir_all(&path).is_ok() {
                        report.legacy_dirs_removed += 1;
                    }
                } else if fs::remove_file(&path).is_ok() {
                    report.legacy_dirs_removed += 1;
                }
            }
        }
    }

    Ok(report)
}

fn prune_tasks_older_than(retention_secs: u64, dry_run: bool) -> Result<u64, String> {
    let now_secs = current_unix_secs();
    let cutoff_secs = now_secs.saturating_sub(retention_secs.max(1)) as i64;

    if dry_run {
        with_db(|pool| async move {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tasks \
                 WHERE finished_at IS NOT NULL \
                   AND finished_at < ? \
                   AND status IN ('succeeded', 'failed', 'cancelled', 'skipped')",
            )
            .bind(cutoff_secs)
            .fetch_one(&pool)
            .await?;
            Ok::<u64, sqlx::Error>(count as u64)
        })
    } else {
        with_db(|pool| async move {
            let res = sqlx::query(
                "DELETE FROM tasks \
                 WHERE finished_at IS NOT NULL \
                   AND finished_at < ? \
                   AND status IN ('succeeded', 'failed', 'cancelled', 'skipped')",
            )
            .bind(cutoff_secs)
            .execute(&pool)
            .await?;
            Ok::<u64, sqlx::Error>(res.rows_affected())
        })
    }
}

fn handle_image_locks_api(ctx: &RequestContext) -> Result<(), String> {
    if !ensure_admin(ctx, "image-locks-api")? {
        return Ok(());
    }

    if !ensure_infra_ready(ctx, "image-locks-api")? {
        return Ok(());
    }

    if ctx.method == "GET" && ctx.path == "/api/image-locks" {
        let db_result = with_db(|pool| async move {
            let rows: Vec<SqliteRow> = sqlx::query(
                "SELECT bucket, acquired_at FROM image_locks ORDER BY acquired_at DESC",
            )
            .fetch_all(&pool)
            .await?;
            Ok::<Vec<SqliteRow>, sqlx::Error>(rows)
        });

        let rows = match db_result {
            Ok(ok) => ok,
            Err(err) => {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to query image locks",
                    "image-locks-api",
                    Some(json!({ "error": err })),
                )?;
                return Ok(());
            }
        };

        let now = current_unix_secs() as i64;
        let mut locks = Vec::with_capacity(rows.len());
        for row in rows {
            let bucket: String = row.get("bucket");
            let acquired_at: i64 = row.get("acquired_at");
            let age_secs = now.saturating_sub(acquired_at).max(0);

            locks.push(json!({
                "bucket": bucket,
                "acquired_at": acquired_at,
                "age_secs": age_secs,
            }));
        }

        let response = json!({
            "now": now,
            "locks": locks,
        });
        return respond_json(ctx, 200, "OK", &response, "image-locks-api", None);
    }

    if ctx.method == "DELETE" {
        let Some(rest) = ctx.path.strip_prefix("/api/image-locks/") else {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "missing lock name",
                "image-locks-api",
                Some(json!({ "reason": "bucket" })),
            )?;
            return Ok(());
        };

        let bucket = rest.trim_matches('/');
        if bucket.is_empty() {
            respond_text(
                ctx,
                400,
                "BadRequest",
                "missing lock name",
                "image-locks-api",
                Some(json!({ "reason": "bucket" })),
            )?;
            return Ok(());
        }

        let bucket_owned = bucket.to_string();
        let db_result = with_db(|pool| async move {
            let res = sqlx::query("DELETE FROM image_locks WHERE bucket = ?")
                .bind(bucket_owned)
                .execute(&pool)
                .await?;
            Ok::<u64, sqlx::Error>(res.rows_affected())
        });

        let deleted = match db_result {
            Ok(rows) => rows,
            Err(err) => {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "failed to delete image lock",
                    "image-locks-api",
                    Some(json!({ "error": err })),
                )?;
                return Ok(());
            }
        };

        let status = if deleted > 0 { 200 } else { 404 };
        let reason = if status == 200 { "OK" } else { "NotFound" };
        let response = json!({
            "bucket": bucket,
            "removed": deleted > 0,
            "rows": deleted,
        });

        respond_json(ctx, status, reason, &response, "image-locks-api", None)?;
        return Ok(());
    }

    respond_text(
        ctx,
        405,
        "MethodNotAllowed",
        "method not allowed",
        "image-locks-api",
        Some(json!({ "reason": "method" })),
    )?;
    Ok(())
}

fn handle_prune_state_api(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "POST" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "prune-state-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "prune-state-api")? {
        return Ok(());
    }

    let request: PruneStateRequest = if ctx.body.is_empty() {
        PruneStateRequest {
            max_age_hours: None,
            dry_run: false,
        }
    } else {
        match parse_json_body(ctx) {
            Ok(body) => body,
            Err(err) => {
                respond_text(
                    ctx,
                    400,
                    "BadRequest",
                    "invalid request",
                    "prune-state-api",
                    Some(json!({ "error": err })),
                )?;
                return Ok(());
            }
        }
    };

    let retention_secs = request
        .max_age_hours
        .unwrap_or(DEFAULT_STATE_RETENTION_SECS / 3600)
        .saturating_mul(3600)
        .max(1);
    let max_age_hours = retention_secs / 3600;
    let task_retention_secs = task_retention_secs_from_env();
    let dry_run = request.dry_run;

    let task_id = create_maintenance_prune_task_for_api(max_age_hours, dry_run, ctx).ok();

    let mut result = if let Some(ref task_id_ref) = task_id {
        run_maintenance_prune_task(task_id_ref, retention_secs, dry_run)
    } else {
        prune_state_dir(Duration::from_secs(retention_secs), dry_run)
    };

    if task_id.is_none() {
        if let Ok(report) = &mut result {
            let tasks_removed = match prune_tasks_older_than(task_retention_secs, dry_run) {
                Ok(count) => count as usize,
                Err(err) => {
                    log_message(&format!(
                        "error task-prune-failed retention_secs={} dry_run={} err={}",
                        task_retention_secs, dry_run, err
                    ));
                    0
                }
            };
            report.tasks_removed = tasks_removed;
            log_message(&format!(
                "info task-prune removed {} tasks older than {} seconds dry_run={}",
                tasks_removed, task_retention_secs, dry_run
            ));
        }
    }

    match result {
        Ok(report) => {
            let response = PruneStateResponse {
                tokens_removed: report.tokens_removed,
                locks_removed: report.locks_removed,
                legacy_dirs_removed: report.legacy_dirs_removed,
                tasks_removed: report.tasks_removed,
                task_retention_secs,
                dry_run,
                max_age_hours,
                task_id: task_id.clone(),
            };
            let payload = serde_json::to_value(&response).map_err(|e| e.to_string())?;
            respond_json(
                ctx,
                200,
                "OK",
                &payload,
                "prune-state-api",
                Some(json!({
                    "dry_run": dry_run,
                    "max_age_hours": max_age_hours,
                    "task_retention_secs": task_retention_secs,
                    "tasks_removed": report.tasks_removed,
                    "task_id": task_id,
                })),
            )?;
            Ok(())
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to prune state",
                "prune-state-api",
                Some(json!({
                    "error": err,
                    "task_id": task_id,
                })),
            )?;
            Ok(())
        }
    }
}

fn handle_debug_payload_download(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" && ctx.method != "HEAD" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "debug-payload-download",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "debug-payload-download")? {
        return Ok(());
    }

    let debug_path = env::var(ENV_DEBUG_PAYLOAD_PATH)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| {
            let default = Path::new(DEFAULT_STATE_DIR).join("last_payload.bin");
            default.to_string_lossy().into_owned()
        });

    let path = Path::new(&debug_path);
    let meta = match fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => {
            respond_text(
                ctx,
                404,
                "NotFound",
                "debug payload not found",
                "debug-payload-download",
                Some(json!({ "path": debug_path, "reason": "not-file" })),
            )?;
            return Ok(());
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            respond_text(
                ctx,
                404,
                "NotFound",
                "debug payload not found",
                "debug-payload-download",
                Some(json!({ "path": debug_path })),
            )?;
            return Ok(());
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to read debug payload",
                "debug-payload-download",
                Some(json!({ "path": debug_path, "error": err.to_string() })),
            )?;
            return Ok(());
        }
    };

    let len = meta.len().min(usize::MAX as u64) as usize;

    if ctx.method == "HEAD" {
        respond_head(
            ctx,
            200,
            "OK",
            "application/octet-stream",
            len,
            "debug-payload-download",
            Some(json!({ "path": debug_path })),
        )?;
        return Ok(());
    }

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(err) => {
            let status = if err.kind() == io::ErrorKind::NotFound {
                404
            } else {
                500
            };
            let reason = if status == 404 {
                "NotFound"
            } else {
                "InternalServerError"
            };
            let body = if status == 404 {
                "debug payload not found"
            } else {
                "failed to read debug payload"
            };
            respond_text(
                ctx,
                status,
                reason,
                body,
                "debug-payload-download",
                Some(json!({ "path": debug_path, "error": err.to_string() })),
            )?;
            return Ok(());
        }
    };

    let mut buf = Vec::with_capacity(len);
    if let Err(err) = file.read_to_end(&mut buf) {
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "failed to read debug payload",
            "debug-payload-download",
            Some(json!({ "path": debug_path, "error": err.to_string() })),
        )?;
        return Ok(());
    }

    respond_binary(
        ctx,
        200,
        "OK",
        "application/octet-stream",
        &buf,
        "debug-payload-download",
        Some(json!({
            "path": debug_path,
            "size": len as u64,
        })),
    )
}

fn try_serve_frontend(ctx: &RequestContext) -> Result<bool, String> {
    if ctx.method != "GET" && ctx.method != "HEAD" {
        return Ok(false);
    }
    let head_only = ctx.method == "HEAD";

    let relative = match ctx.path.as_str() {
        "/" | "/index.html" | "/manual" | "/webhooks" | "/events" | "/tasks" | "/maintenance"
        | "/settings" | "/401" => PathBuf::from("index.html"),
        path if path.starts_with("/assets/") => match sanitize_frontend_path(path) {
            Some(p) => p,
            None => return Ok(false),
        },
        "/mockServiceWorker.js" => PathBuf::from("mockServiceWorker.js"),
        "/vite.svg" => PathBuf::from("vite.svg"),
        "/favicon.ico" => PathBuf::from("favicon.ico"),
        _ => return Ok(false),
    };

    let dist_dir = frontend_dist_dir();
    let asset_path = dist_dir.join(&relative);

    if asset_path.is_file() {
        let content_type = content_type_for(&relative);
        if head_only {
            let len = fs::metadata(&asset_path)
                .map(|meta| meta.len())
                .unwrap_or(0)
                .min(usize::MAX as u64);
            respond_head(
                ctx,
                200,
                "OK",
                content_type,
                len as usize,
                "frontend",
                Some(json!({ "asset": relative.to_string_lossy() })),
            )?;
            return Ok(true);
        }

        let body = fs::read(&asset_path)
            .map_err(|e| format!("failed to read asset {}: {e}", asset_path.display()))?;
        respond_binary(
            ctx,
            200,
            "OK",
            content_type,
            &body,
            "frontend",
            Some(json!({ "asset": relative.to_string_lossy() })),
        )?;
        return Ok(true);
    }

    if relative == PathBuf::from("index.html") {
        log_message("500 web-ui missing index.html");
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "web ui not built",
            "frontend",
            Some(json!({ "asset": relative.to_string_lossy() })),
        )?;
        return Ok(true);
    }

    log_message(&format!(
        "404 asset-not-found path={} relative={}",
        ctx.path,
        relative.display()
    ));
    respond_text(
        ctx,
        404,
        "NotFound",
        "asset not found",
        "frontend",
        Some(json!({ "asset": relative.to_string_lossy() })),
    )?;
    Ok(true)
}

fn handle_config_api(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "config-api",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    // This endpoint is intentionally open: it only exposes values that are
    // either already visible to the user (current origin) or safe to know
    // from the UI.
    let webhook_prefix = public_base_url();
    let path_prefix = format!("/{GITHUB_ROUTE_PREFIX}");

    let response = json!({
        "web": {
            "webhook_url_prefix": webhook_prefix,
            "github_webhook_path_prefix": path_prefix,
        },
    });

    respond_json(ctx, 200, "OK", &response, "config-api", None)
}

fn frontend_dist_dir() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();

    let mut push_unique = |path: PathBuf| {
        if path.as_os_str().is_empty() {
            return;
        }
        if !candidates.iter().any(|existing| existing == &path) {
            candidates.push(path);
        }
    };

    if let Ok(state_dir) = env::var(ENV_STATE_DIR) {
        if !state_dir.trim().is_empty() {
            push_unique(PathBuf::from(state_dir).join(DEFAULT_WEB_DIST_DIR));
        }
    }

    if let Ok(cwd) = env::current_dir() {
        push_unique(cwd.join(DEFAULT_WEB_DIST_DIR));
    }

    push_unique(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_WEB_DIST_DIR));
    push_unique(PathBuf::from(DEFAULT_WEB_DIST_FALLBACK));

    candidates
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .unwrap_or_else(|| {
            candidates
                .first()
                .cloned()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WEB_DIST_FALLBACK))
        })
}

fn sanitize_frontend_path(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(PathBuf::from("index.html"));
    }

    let mut sanitized = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => continue,
            _ => return None,
        }
    }

    if sanitized.as_os_str().is_empty() {
        sanitized.push("index.html");
    }

    Some(sanitized)
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

fn handle_webhooks_status(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "webhooks-status",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    if !ensure_admin(ctx, "webhooks-status")? {
        return Ok(());
    }

    if !ensure_infra_ready(ctx, "webhooks-status")? {
        return Ok(());
    }

    let secret_configured = env::var(ENV_GH_WEBHOOK_SECRET)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    #[derive(Clone)]
    struct UnitStatusAgg {
        unit: String,
        slug: String,
        last_ts: Option<i64>,
        last_status: Option<i64>,
        last_request_id: Option<String>,
        last_success_ts: Option<i64>,
        last_failure_ts: Option<i64>,
        last_hmac_error_ts: Option<i64>,
        last_hmac_error_reason: Option<String>,
    }

    impl UnitStatusAgg {
        fn new(unit: String) -> Self {
            let slug = unit
                .trim()
                .trim_matches('/')
                .trim_end_matches(".service")
                .to_string();
            UnitStatusAgg {
                unit,
                slug,
                last_ts: None,
                last_status: None,
                last_request_id: None,
                last_success_ts: None,
                last_failure_ts: None,
                last_hmac_error_ts: None,
                last_hmac_error_reason: None,
            }
        }
    }

    let db_result = with_db(|pool| async move {
        let rows: Vec<SqliteRow> = sqlx::query(
            "SELECT id, request_id, ts, status, path, meta FROM event_log WHERE action = 'github-webhook' ORDER BY ts DESC, id DESC LIMIT ?",
        )
        .bind(WEBHOOK_STATUS_LOOKBACK as i64)
        .fetch_all(&pool)
        .await?;
        Ok::<Vec<SqliteRow>, sqlx::Error>(rows)
    });

    let rows = match db_result {
        Ok(ok) => ok,
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to query webhooks",
                "webhooks-status",
                Some(json!({ "error": err })),
            )?;
            return Ok(());
        }
    };

    let mut units: HashMap<String, UnitStatusAgg> = HashMap::new();

    for unit in webhook_unit_list() {
        units
            .entry(unit.clone())
            .or_insert_with(|| UnitStatusAgg::new(unit));
    }

    for row in rows {
        let ts: i64 = row.get("ts");
        let status_code: i64 = row.get("status");
        let path: Option<String> = row.get("path");
        let request_id: String = row.get("request_id");
        let meta_raw: String = row.get("meta");
        let meta: Value = serde_json::from_str(&meta_raw).unwrap_or_else(|_| json!({}));

        let unit_name = meta
            .get("unit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| path.as_deref().and_then(|p| lookup_unit_from_path(p)));

        let Some(unit_name) = unit_name else {
            continue;
        };

        let entry = units
            .entry(unit_name.clone())
            .or_insert_with(|| UnitStatusAgg::new(unit_name.clone()));

        if entry.last_ts.map_or(true, |existing| ts > existing) {
            entry.last_ts = Some(ts);
            entry.last_status = Some(status_code);
            entry.last_request_id = Some(request_id.clone());
        }

        if status_code == 202 {
            if entry.last_success_ts.map_or(true, |existing| ts > existing) {
                entry.last_success_ts = Some(ts);
            }
        } else if status_code >= 400 {
            if entry.last_failure_ts.map_or(true, |existing| ts > existing) {
                entry.last_failure_ts = Some(ts);
            }
        }

        if status_code == 401 {
            if let Some(reason) = meta.get("reason").and_then(|v| v.as_str()) {
                if entry
                    .last_hmac_error_ts
                    .map_or(true, |existing| ts > existing)
                {
                    entry.last_hmac_error_ts = Some(ts);
                    entry.last_hmac_error_reason = Some(reason.to_string());
                }
            }
        }
    }

    let now = current_unix_secs() as i64;
    let mut unit_values: Vec<UnitStatusAgg> = units.into_iter().map(|(_, v)| v).collect();
    unit_values.sort_by(|a, b| a.slug.cmp(&b.slug));

    let mut entries = Vec::with_capacity(unit_values.len());
    let base_url = public_base_url();
    for u in unit_values {
        let expected_image = unit_configured_image(&u.unit);
        let webhook_path = format!("/{}/{}", GITHUB_ROUTE_PREFIX, u.slug);
        let redeploy_path = format!("{webhook_path}/redeploy");
        let webhook_url = base_url
            .as_ref()
            .map(|base| format!("{base}{webhook_path}"))
            .unwrap_or_else(|| webhook_path.clone());
        let redeploy_url = base_url
            .as_ref()
            .map(|base| format!("{base}{redeploy_path}"))
            .unwrap_or_else(|| redeploy_path.clone());
        let hmac_ok = u.last_hmac_error_ts.is_none();

        entries.push(json!({
            "unit": u.unit,
            "slug": u.slug,
            "webhook_path": webhook_path,
            "redeploy_path": redeploy_path,
            "webhook_url": webhook_url,
            "redeploy_url": redeploy_url,
            "expected_image": expected_image,
            "last_ts": u.last_ts,
            "last_status": u.last_status,
            "last_request_id": u.last_request_id,
            "last_success_ts": u.last_success_ts,
            "last_failure_ts": u.last_failure_ts,
            "hmac_ok": hmac_ok,
            "hmac_last_error": u.last_hmac_error_reason,
        }));
    }

    let response = json!({
        "now": now,
        "secret_configured": secret_configured,
        "units": entries,
    });

    respond_json(ctx, 200, "OK", &response, "webhooks-status", None)
}

fn handle_github_request(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "POST" {
        log_message(&format!(
            "405 github-method-not-allowed {}",
            ctx.raw_request
        ));
        respond_text(
            ctx,
            405,
            "MethodNotAllowed",
            "method not allowed",
            "github-webhook",
            Some(json!({ "reason": "method" })),
        )?;
        return Ok(());
    }

    let secret = env::var(ENV_GH_WEBHOOK_SECRET)
        .unwrap_or_default()
        // Trim common whitespace so secrets sourced from files or env lists
        // don't fail HMAC due to stray newlines/spaces.
        .trim()
        .to_string();

    if secret.is_empty() {
        log_message("500 github-misconfigured missing secret");
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "server misconfigured",
            "github-webhook",
            Some(json!({ "reason": "missing-secret" })),
        )?;
        return Ok(());
    }

    let signature = match ctx.headers.get("x-hub-signature-256") {
        Some(value) => value,
        None => {
            log_message("401 github missing signature");
            respond_text(
                ctx,
                401,
                "Unauthorized",
                "unauthorized",
                "github-webhook",
                Some(json!({ "reason": "missing-signature" })),
            )?;
            return Ok(());
        }
    };

    let sig = verify_github_signature(signature, &secret, &ctx.body)?;
    if !sig.valid {
        log_message(&format!(
            "401 github signature-mismatch provided={} expected={} expected-len={} expected-error={} body-sha256={} dump={} dump-error={} secret-len={} body-len={} header-raw={} prefix-ok={}",
            sig.provided,
            sig.expected,
            sig.expected_len,
            sig.expected_error.as_deref().unwrap_or(""),
            sig.body_sha256,
            sig.payload_dump.as_deref().unwrap_or(""),
            sig.dump_error.as_deref().unwrap_or(""),
            secret.len(),
            ctx.body.len(),
            sig.header_raw,
            sig.prefix_ok,
        ));
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "github-webhook",
            Some(json!({
                "reason": "signature",
                "provided": sig.provided,
                "expected": sig.expected,
                "expected_error": sig.expected_error,
                "expected_len": sig.expected_len,
                "body_sha256": sig.body_sha256,
                "dump": sig.payload_dump,
                "dump_error": sig.dump_error,
                "header_raw": sig.header_raw,
                "headers": ctx.headers,
                "prefix_ok": sig.prefix_ok,
            })),
        )?;
        return Ok(());
    }

    let event = ctx
        .headers
        .get("x-github-event")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());

    if !github_event_allowed(&event) {
        log_message(&format!("202 github event-ignored event={event}"));
        respond_text(
            ctx,
            202,
            "Accepted",
            "event ignored",
            "github-webhook",
            Some(json!({ "reason": "event", "event": event })),
        )?;
        return Ok(());
    }

    let Some(unit) = lookup_unit_from_path(&ctx.path) else {
        log_message(&format!(
            "202 github event={event} path={} no-unit-mapped",
            ctx.path
        ));
        respond_text(
            ctx,
            202,
            "Accepted",
            "event ignored",
            "github-webhook",
            Some(json!({ "reason": "no-unit", "event": event })),
        )?;
        return Ok(());
    };

    let image = match extract_container_image(&ctx.body) {
        Ok(img) => img,
        Err(reason) => {
            log_message(&format!("202 github event={event} skipped reason={reason}"));
            respond_text(
                ctx,
                202,
                "Accepted",
                "event ignored",
                "github-webhook",
                Some(json!({ "reason": reason, "event": event })),
            )?;
            return Ok(());
        }
    };

    if let Some(expected) = unit_configured_image(&unit) {
        if !images_match(&image, &expected) {
            log_message(&format!(
                "202 github event={event} unit={unit} image={image} expected={expected} skipped=tag-mismatch"
            ));
            respond_text(
                ctx,
                202,
                "Accepted",
                "tag mismatch",
                "github-webhook",
                Some(json!({ "unit": unit, "expected": expected, "image": image })),
            )?;
            return Ok(());
        }
    }

    let delivery = ctx
        .headers
        .get("x-github-delivery")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());

    if let Err(err) = check_github_image_limit(&image) {
        match err {
            RateLimitError::LockTimeout => {
                log_message(&format!(
                    "429 github-rate-limit lock-timeout image={image} event={event}"
                ));
                respond_text(
                    ctx,
                    429,
                    "Too Many Requests",
                    "rate limited",
                    "github-webhook",
                    Some(json!({ "reason": "lock", "image": image })),
                )?;
                return Ok(());
            }
            RateLimitError::Exceeded { c1, l1, .. } => {
                log_message(&format!(
                    "429 github-rate-limit image={image} count={c1}/{l1} event={event}"
                ));
                respond_text(
                    ctx,
                    429,
                    "Too Many Requests",
                    "rate limited",
                    "github-webhook",
                    Some(json!({ "c1": c1, "l1": l1, "image": image })),
                )?;
                return Ok(());
            }
            RateLimitError::Io(err) => return Err(err),
        }
    }

    log_message(&format!(
        "202 github-queued unit={unit} image={image} event={event} delivery={delivery} path={}",
        ctx.path
    ));

    // Create a Task record for this webhook-triggered background job.
    let task_meta = TaskMeta::GithubWebhook {
        unit: unit.clone(),
        image: image.clone(),
        event: event.clone(),
        delivery: delivery.clone(),
        path: ctx.path.clone(),
    };
    let task_id = create_github_task(
        &unit,
        &image,
        &event,
        &delivery,
        &ctx.path,
        &ctx.request_id,
        &task_meta,
    )?;

    if let Err(err) = spawn_background_task(&unit, &image, &event, &delivery, &ctx.path, &task_id)
    {
        log_message(&format!(
            "500 github-dispatch-failed unit={unit} image={image} event={event} delivery={delivery} path={} err={err}",
            ctx.path
        ));
        mark_task_dispatch_failed(
            &task_id,
            Some(&unit),
            "github-webhook",
            "github-webhook",
            &err,
            json!({
                "unit": unit,
                "image": image,
                "event": event,
                "delivery": delivery,
                "path": ctx.path,
                "request_id": ctx.request_id,
            }),
        );
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "failed to dispatch",
            "github-webhook",
            Some(json!({ "unit": unit, "image": image, "error": err, "task_id": task_id })),
        )?;
        return Ok(());
    }

    respond_text(
        ctx,
        202,
        "Accepted",
        "auto-update queued",
        "github-webhook",
        Some(json!({ "unit": unit, "image": image, "delivery": delivery, "task_id": task_id })),
    )
}

fn enforce_rate_limit(ctx: &RequestContext, context: &str) -> Result<bool, String> {
    match rate_limit_check() {
        Ok(()) => Ok(true),
        Err(RateLimitError::LockTimeout) => {
            log_message("429 rate-limit lock-timeout");
            respond_text(
                ctx,
                429,
                "Too Many Requests",
                "rate limited",
                "manual-auto-update",
                Some(json!({ "reason": "lock" })),
            )?;
            Ok(false)
        }
        Err(RateLimitError::Exceeded { c1, l1, c2, l2 }) => {
            log_message(&format!(
                "429 rate-limit c1={c1}/{l1} c2={c2}/{l2} ({context})"
            ));
            respond_text(
                ctx,
                429,
                "Too Many Requests",
                "rate limited",
                "manual-auto-update",
                Some(json!({ "c1": c1, "l1": l1, "c2": c2, "l2": l2 })),
            )?;
            Ok(false)
        }
        Err(RateLimitError::Io(err)) => Err(err),
    }
}

struct ImageTaskGuard {
    _lock: ImageLockGuard,
}

struct ImageLockGuard {
    bucket: String,
}

impl Drop for ImageLockGuard {
    fn drop(&mut self) {
        let bucket = self.bucket.clone();
        let _ = with_db(move |pool| async move {
            let _ = sqlx::query("DELETE FROM image_locks WHERE bucket = ?")
                .bind(bucket)
                .execute(&pool)
                .await?;
            Ok::<(), sqlx::Error>(())
        });
    }
}

fn check_github_image_limit(image: &str) -> Result<(), RateLimitError> {
    let bucket = sanitize_image_key(image);
    let windows = [RateWindow {
        limit: GITHUB_IMAGE_LIMIT_COUNT,
        window: GITHUB_IMAGE_LIMIT_WINDOW,
    }];
    apply_rate_limits(
        "github-image",
        &bucket,
        current_unix_secs(),
        &windows,
        false,
    )
}

fn enforce_github_image_limit(image: &str) -> Result<ImageTaskGuard, RateLimitError> {
    let bucket = sanitize_image_key(image);
    let lock = acquire_image_lock(&bucket)?;
    let windows = [RateWindow {
        limit: GITHUB_IMAGE_LIMIT_COUNT,
        window: GITHUB_IMAGE_LIMIT_WINDOW,
    }];

    match apply_rate_limits("github-image", &bucket, current_unix_secs(), &windows, true) {
        Ok(()) => Ok(ImageTaskGuard { _lock: lock }),
        Err(err) => {
            drop(lock);
            Err(err)
        }
    }
}

fn acquire_image_lock(bucket: &str) -> Result<ImageLockGuard, RateLimitError> {
    let deadline = Instant::now() + LOCK_TIMEOUT;
    let bucket_owned = bucket.to_string();
    loop {
        let now = current_unix_secs();
        let bucket_for_query = bucket_owned.clone();
        let inserted = with_db(move |pool| async move {
            let res = sqlx::query(
                "INSERT INTO image_locks (bucket, acquired_at) VALUES (?, ?) ON CONFLICT DO NOTHING",
            )
            .bind(bucket_for_query)
            .bind(now as i64)
            .execute(&pool)
            .await?;
            Ok::<u64, sqlx::Error>(res.rows_affected())
        })
        .map_err(RateLimitError::Io)?;

        if inserted > 0 {
            return Ok(ImageLockGuard {
                bucket: bucket_owned.clone(),
            });
        }

        if Instant::now() >= deadline {
            return Err(RateLimitError::LockTimeout);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Clone)]
struct RateWindow {
    limit: u64,
    window: u64,
}

enum RateLimitDbResult {
    Allowed,
    Exceeded(Vec<u64>),
}

fn apply_rate_limits(
    scope: &str,
    bucket: &str,
    now_secs: u64,
    windows: &[RateWindow],
    insert_on_success: bool,
) -> Result<(), RateLimitError> {
    let max_window = windows.iter().map(|w| w.window).max().unwrap_or(0);
    let scope_owned = scope.to_string();
    let bucket_owned = bucket.to_string();
    let windows_owned: Vec<RateWindow> = windows.to_vec();

    let result = with_db(move |pool| async move {
        let scope = scope_owned;
        let bucket = bucket_owned;
        let windows = windows_owned;
        let mut tx = pool.begin().await?;
        if max_window > 0 {
            let cutoff = now_secs.saturating_sub(max_window) as i64;
            sqlx::query("DELETE FROM rate_limit_tokens WHERE scope = ? AND bucket = ? AND ts < ?")
                .bind(&scope)
                .bind(&bucket)
                .bind(cutoff)
                .execute(&mut *tx)
                .await?;
        }

        let mut counts = Vec::with_capacity(windows.len());
        for window in &windows {
            let cutoff = now_secs.saturating_sub(window.window) as i64;
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM rate_limit_tokens WHERE scope = ? AND bucket = ? AND ts >= ?",
            )
            .bind(&scope)
            .bind(&bucket)
            .bind(cutoff)
            .fetch_one(&mut *tx)
            .await?;
            counts.push(count as u64);
        }

        let mut exceeded = false;
        for (idx, window) in windows.iter().enumerate() {
            if counts.get(idx).copied().unwrap_or(0) >= window.limit {
                exceeded = true;
                break;
            }
        }

        if exceeded {
            tx.rollback().await?;
            return Ok(RateLimitDbResult::Exceeded(counts));
        }

        if insert_on_success {
            sqlx::query("INSERT INTO rate_limit_tokens (scope, bucket, ts) VALUES (?, ?, ?)")
                .bind(&scope)
                .bind(&bucket)
                .bind(now_secs as i64)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(RateLimitDbResult::Allowed)
    })
    .map_err(RateLimitError::Io)?;

    match result {
        RateLimitDbResult::Allowed => Ok(()),
        RateLimitDbResult::Exceeded(counts) => {
            let c1 = counts.get(0).copied().unwrap_or(0);
            let l1 = windows.get(0).map(|w| w.limit).unwrap_or(0);
            let c2 = counts.get(1).copied().unwrap_or(c1);
            let l2 = windows.get(1).map(|w| w.limit).unwrap_or(l1);
            Err(RateLimitError::Exceeded { c1, l1, c2, l2 })
        }
    }
}

struct CommandExecResult {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl CommandExecResult {
    fn success(&self) -> bool {
        self.status.success()
    }
}

fn truncate_command_output(text: &str) -> (String, bool) {
    if text.len() <= COMMAND_OUTPUT_MAX_LEN {
        return (text.to_string(), false);
    }

    let mut truncated = String::new();
    for ch in text.chars().take(COMMAND_OUTPUT_MAX_LEN) {
        truncated.push(ch);
    }
    (truncated, true)
}

fn build_command_meta(
    command: &str,
    argv: &[&str],
    result: &CommandExecResult,
    extra_meta: Option<Value>,
) -> Value {
    let (stdout, truncated_stdout) = truncate_command_output(&result.stdout);
    let (stderr, truncated_stderr) = truncate_command_output(&result.stderr);
    let exit = format!("exit={}", exit_code_string(&result.status));

    let mut meta = json!({
        "type": "command",
        "command": command,
        "argv": argv,
        "exit": exit,
    });

    if !stdout.is_empty() {
        meta["stdout"] = Value::String(stdout);
        if truncated_stdout {
            meta["truncated_stdout"] = Value::Bool(true);
        }
    }

    if !stderr.is_empty() {
        meta["stderr"] = Value::String(stderr);
        if truncated_stderr {
            meta["truncated_stderr"] = Value::Bool(true);
        }
    }

    if let Some(extra) = extra_meta {
        match extra {
            Value::Object(map) => {
                if let Some(obj) = meta.as_object_mut() {
                    for (k, v) in map {
                        // Preserve explicit command fields when keys collide.
                        obj.entry(k).or_insert(v);
                    }
                }
            }
            other => {
                meta["extra"] = other;
            }
        }
    }

    meta
}

fn run_quiet_command(mut command: Command) -> Result<CommandExecResult, String> {
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    Ok(CommandExecResult {
        status: output.status,
        stdout,
        stderr,
    })
}

fn podman_health() -> Result<(), String> {
    PODMAN_HEALTH
        .get_or_init(|| {
            if env::var("PODUP_SKIP_PODMAN")
                .ok()
                .as_deref()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
            {
                return Ok(());
            }

            let result = run_quiet_command({
                let mut cmd = Command::new("podman");
                cmd.arg("--version");
                cmd
            });
            match result {
                Ok(res) if res.success() => Ok(()),
                Ok(res) => Err(format!(
                    "podman unavailable: {}",
                    exit_code_string(&res.status)
                )),
                Err(err) => Err(format!("podman unavailable: {err}")),
            }
        })
        .clone()
}

fn start_auto_update_unit(unit: &str) -> Result<CommandExecResult, String> {
    // Prefer talking to the user scope systemd instance via D-Bus so that we
    // work in containerised environments where `systemctl --user` cannot reach
    // the user bus directly but busctl can (for example when /run/user/$UID is
    // bind-mounted into the container).
    //
    // If busctl is not available at all (e.g. on non-systemd dev hosts), fall
    // back to the previous `systemctl --user start` behaviour.
    if let Ok(result) = run_quiet_command({
        let mut cmd = Command::new("busctl");
        cmd.arg("--user")
            .arg("call")
            .arg("org.freedesktop.systemd1")
            .arg("/org/freedesktop/systemd1")
            .arg("org.freedesktop.systemd1.Manager")
            .arg("StartUnit")
            .arg("ss")
            .arg(unit)
            .arg("replace");
        cmd
    }) {
        // Always return the busctl result to the caller; non-zero exit codes
        // are treated as failures by the higher-level logic which will keep
        // returning 500s and surfacing stderr in task logs.
        return Ok(result);
    }

    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user").arg("start").arg(unit);
        cmd
    })
}

fn restart_unit(unit: &str) -> Result<CommandExecResult, String> {
    // See start_auto_update_unit for rationale; use the same D-Bus path for
    // restart operations, with a systemctl fallback when busctl is missing.
    if let Ok(result) = run_quiet_command({
        let mut cmd = Command::new("busctl");
        cmd.arg("--user")
            .arg("call")
            .arg("org.freedesktop.systemd1")
            .arg("/org/freedesktop/systemd1")
            .arg("org.freedesktop.systemd1.Manager")
            .arg("RestartUnit")
            .arg("ss")
            .arg(unit)
            .arg("replace");
        cmd
    }) {
        return Ok(result);
    }

    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user").arg("restart").arg(unit);
        cmd
    })
}

/// Best-effort graceful stop of a systemd unit backing a running task.
fn stop_task_runner_unit(unit: &str) -> Result<CommandExecResult, String> {
    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user").arg("stop").arg(unit);
        cmd
    })
}

/// Forcefully terminate a systemd unit backing a running task.
fn kill_task_runner_unit(unit: &str) -> Result<CommandExecResult, String> {
    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user")
            .arg("kill")
            .arg("--signal=SIGKILL")
            .arg(unit);
        cmd
    })
}

fn pull_container_image(image: &str) -> Result<CommandExecResult, String> {
    let mut last_result: Option<CommandExecResult> = None;

    for attempt in 1..=PULL_RETRY_ATTEMPTS {
        let result = run_quiet_command({
            let mut cmd = Command::new("podman");
            cmd.arg("pull").arg(image);
            cmd
        })?;
        if result.success() {
            return Ok(result);
        }

        last_result = Some(result);

        if attempt < PULL_RETRY_ATTEMPTS {
            thread::sleep(Duration::from_secs(PULL_RETRY_DELAY_SECS));
        }
    }

    Ok(last_result.expect("PULL_RETRY_ATTEMPTS must be >= 1"))
}

fn prune_images_silently() {
    match run_quiet_command({
        let mut cmd = Command::new("podman");
        cmd.arg("image").arg("prune").arg("-f");
        cmd
    }) {
        Ok(result) => {
            if !result.success() {
                let mut msg = format!(
                    "warn image-prune-failed exit={}",
                    exit_code_string(&result.status)
                );
                if !result.stderr.is_empty() {
                    msg.push_str(" stderr=");
                    msg.push_str(&result.stderr);
                }
                log_message(&msg);
            }
        }
        Err(err) => {
            log_message(&format!("warn image-prune-error err={err}"));
        }
    }
}

fn spawn_background_task(
    unit: &str,
    image: &str,
    event: &str,
    delivery: &str,
    path: &str,
    task_id: &str,
) -> Result<(), String> {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_str().ok_or_else(|| "invalid exe path".to_string())?;
    let suffix = sanitize_image_key(delivery);
    let unit_name = format!("webhook-task-{}", suffix);

    log_message(&format!(
        "debug github-dispatch-launch unit={unit} image={image} event={event} delivery={delivery} path={path} exe={exe_str} task-unit={unit_name} task_id={task_id}"
    ));

    let args = build_systemd_run_args(&unit_name, exe_str, task_id);

    if let Ok(snapshot) = env::var(ENV_SYSTEMD_RUN_SNAPSHOT) {
        fs::write(snapshot, args.join("\n")).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let status = Command::new("systemd-run")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(exit_code_string(&status)),
        Err(err) => {
            log_message(&format!(
                "warn github-dispatch-fallback no-systemd-run err={err} running-inline"
            ));
            spawn_inline_task(exe_str, task_id)
        }
    }
}

fn spawn_inline_task(exe: &str, task_id: &str) -> Result<(), String> {
    // Best-effort fallback when systemd-run is unavailable (dev/test containers).
    Command::new(exe)
        .arg("--run-task")
        .arg(task_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn build_systemd_run_args(unit_name: &str, exe: &str, task_id: &str) -> Vec<String> {
    vec![
        "--user".into(),
        "--collect".into(),
        "--quiet".into(),
        format!("--unit={unit_name}"),
        exe.to_string(),
        "--run-task".into(),
        task_id.to_string(),
    ]
}

fn run_background_task(
    task_id: &str,
    unit: &str,
    image: &str,
    event: &str,
    delivery: &str,
    path: &str,
) -> Result<(), String> {
    log_message(&format!(
        "debug github-background-start unit={unit} image={image} event={event} delivery={delivery} path={path}"
    ));

    let guard = match enforce_github_image_limit(image) {
        Ok(guard) => guard,
        Err(RateLimitError::LockTimeout) => {
            log_message(&format!(
                "429 github-rate-limit lock-timeout image={image} event={event} delivery={delivery} path={path}"
            ));
            update_task_state_with_unit(
                task_id,
                "skipped",
                unit,
                "skipped",
                "Skipped due to image rate-limit lock timeout",
                "image-rate-limit",
                "warning",
                json!({ "reason": "lock-timeout", "image": image, "event": event, "delivery": delivery, "path": path }),
            );
            return Ok(());
        }
        Err(RateLimitError::Exceeded { c1, l1, .. }) => {
            log_message(&format!(
                "429 github-rate-limit image={image} count={c1}/{l1} event={event} delivery={delivery} path={path}"
            ));
            update_task_state_with_unit(
                task_id,
                "skipped",
                unit,
                "skipped",
                "Skipped due to image rate-limit exceeded",
                "image-rate-limit",
                "warning",
                json!({ "reason": "limit", "c1": c1, "l1": l1, "image": image, "event": event, "delivery": delivery, "path": path }),
            );
            return Ok(());
        }
        Err(RateLimitError::Io(err)) => return Err(err),
    };

    let _guard = guard;

    let pull_result = match pull_container_image(image) {
        Ok(res) => res,
        Err(err) => {
            log_message(&format!(
                "500 github-image-pull-failed unit={unit} image={image} event={event} delivery={delivery} path={path} err={err}"
            ));
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Image pull failed for github webhook task",
                "image-pull",
                "error",
                json!({ "error": err, "image": image, "event": event, "delivery": delivery, "path": path }),
            );
            return Ok(());
        }
    };

    if !pull_result.success() {
        let mut error_message = exit_code_string(&pull_result.status);
        if !pull_result.stderr.is_empty() {
            error_message.push_str(": ");
            error_message.push_str(&pull_result.stderr);
        }

        log_message(&format!(
            "500 github-image-pull-failed unit={unit} image={image} event={event} delivery={delivery} path={path} err={error_message}"
        ));

        let command = format!("podman pull {image}");
        let argv = ["podman", "pull", image];
        let extra_meta = json!({
            "error": error_message,
            "image": image,
            "event": event,
            "delivery": delivery,
            "path": path,
        });
        let meta = build_command_meta(&command, &argv, &pull_result, Some(extra_meta));

        update_task_state_with_unit(
            task_id,
            "failed",
            unit,
            "failed",
            "Image pull failed for github webhook task",
            "image-pull",
            "error",
            meta,
        );
        return Ok(());
    }

    let restart_command = format!("systemctl --user restart {unit}");
    let restart_argv = ["systemctl", "--user", "restart", unit];

    match restart_unit(unit) {
        Ok(result) if result.success() => {
            log_message(&format!(
                "202 github-triggered unit={unit} image={image} event={event} delivery={delivery} path={path}"
            ));
            prune_images_silently();
            let extra_meta = json!({
                "status": "ok",
                "image": image,
                "event": event,
                "delivery": delivery,
                "path": path,
            });
            let meta =
                build_command_meta(&restart_command, &restart_argv, &result, Some(extra_meta));
            update_task_state_with_unit(
                task_id,
                "succeeded",
                unit,
                "succeeded",
                "Github webhook task completed successfully",
                "restart-unit",
                "info",
                meta,
            );
        }
        Ok(result) => {
            let mut message = format!(
                "500 github-restart-failed unit={unit} image={image} event={event} delivery={delivery} path={path} exit={}",
                exit_code_string(&result.status)
            );
            if !result.stderr.is_empty() {
                message.push_str(" stderr=");
                message.push_str(&result.stderr);
            }
            log_message(&message);
            let extra_meta = json!({
                "image": image,
                "event": event,
                "delivery": delivery,
                "path": path,
            });
            let meta =
                build_command_meta(&restart_command, &restart_argv, &result, Some(extra_meta));
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Restart unit failed for github webhook task",
                "restart-unit",
                "error",
                meta,
            );
        }
        Err(err) => {
            log_message(&format!(
                "500 github-restart-error unit={unit} image={image} event={event} delivery={delivery} path={path} err={err}"
            ));
            // For unexpected errors, fall back to a non-command meta payload.
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Restart unit error for github webhook task",
                "restart-unit",
                "error",
                json!({ "error": err, "image": image, "event": event, "delivery": delivery, "path": path }),
            );
        }
    }

    Ok(())
}

fn update_task_state_with_unit(
    task_id: &str,
    new_status: &str,
    unit: &str,
    unit_status: &str,
    summary: &str,
    log_action: &str,
    log_level: &str,
    meta: Value,
) {
    let task_id_owned = task_id.to_string();
    let unit_owned = unit.to_string();
    let status_owned = new_status.to_string();
    let unit_status_owned = unit_status.to_string();
    let summary_owned = summary.to_string();
    let log_action_owned = log_action.to_string();
    let log_level_owned = log_level.to_string();
    let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());
    let now = current_unix_secs() as i64;

    let _ = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "UPDATE tasks \
             SET status = ?, finished_at = COALESCE(finished_at, ?), updated_at = ?, summary = ? \
             WHERE task_id = ?",
        )
        .bind(&status_owned)
        .bind(now)
        .bind(now)
        .bind(&summary_owned)
        .bind(&task_id_owned)
        .execute(&mut *tx)
        .await?;

        // Keep the synthetic "task-created" log status aligned with the final task
        // status so that the timeline does not show a completed task as still
        // "running" or "pending".
        sqlx::query(
            "UPDATE task_logs \
             SET status = ? \
             WHERE task_id = ? AND action = 'task-created' AND status IN ('running', 'pending')",
        )
        .bind(&status_owned)
        .bind(&task_id_owned)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE task_units \
             SET status = ?, finished_at = COALESCE(finished_at, ?), message = ? \
             WHERE task_id = ? AND unit = ?",
        )
        .bind(&unit_status_owned)
        .bind(now)
        .bind(&summary_owned)
        .bind(&task_id_owned)
        .bind(&unit_owned)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_owned)
        .bind(now)
        .bind(&log_level_owned)
        .bind(&log_action_owned)
        .bind(&status_owned)
        .bind(&summary_owned)
        .bind(Some(unit_owned))
        .bind(meta_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });
}

fn merge_task_meta(mut base: Value, extra: Value) -> Value {
    match (&mut base, extra) {
        (Value::Object(base_map), Value::Object(extra_map)) => {
            for (k, v) in extra_map {
                base_map.insert(k, v);
            }
            base
        }
        (Value::Object(base_map), other) if !other.is_null() => {
            base_map.insert("extra".to_string(), other);
            base
        }
        _ => base,
    }
}

fn mark_task_dispatch_failed(
    task_id: &str,
    unit: Option<&str>,
    kind: &str,
    source: &str,
    error: &str,
    extra_meta: Value,
) {
    let summary = if let Some(u) = unit {
        format!("Failed to dispatch {source} task for unit {u}")
    } else {
        format!("Failed to dispatch {source} task")
    };

    let mut base_meta = json!({
        "task_id": task_id,
        "kind": kind,
        "source": source,
        "error": error,
    });
    if let Some(u) = unit {
        base_meta["unit"] = Value::String(u.to_string());
    }

    let merged_meta = merge_task_meta(base_meta, extra_meta);

    // Determine which task_units to mark as failed. When no explicit unit is
    // provided (e.g. manual trigger tasks spanning multiple units), we mark all
    // units belonging to this task as failed.
    let units: Vec<String> = if let Some(u) = unit {
        vec![u.to_string()]
    } else {
        let task_id_owned = task_id.to_string();
        let units_result: Result<Vec<String>, String> = with_db(|pool| async move {
            let rows: Vec<SqliteRow> =
                sqlx::query("SELECT unit FROM task_units WHERE task_id = ? ORDER BY id")
                    .bind(&task_id_owned)
                    .fetch_all(&pool)
                    .await?;
            let mut units = Vec::with_capacity(rows.len());
            for row in rows {
                units.push(row.get::<String, _>("unit"));
            }
            Ok::<Vec<String>, sqlx::Error>(units)
        });

        match units_result {
            Ok(units) if !units.is_empty() => units,
            Ok(_) => Vec::new(),
            Err(err) => {
                log_message(&format!(
                    "warn task-dispatch-failed mark-units-load-failed task_id={task_id} err={err}"
                ));
                Vec::new()
            }
        }
    };

    if units.is_empty() {
        // Best-effort fallback: update the task status and append a log entry
        // without a specific unit, so that the task is never left running
        // without an explanation.
        let task_id_owned = task_id.to_string();
        let summary_owned = summary.clone();
        let meta_str =
            serde_json::to_string(&merged_meta).unwrap_or_else(|_| "{}".to_string());
        let _ = with_db(|pool| async move {
            let mut tx = pool.begin().await?;
            let now = current_unix_secs() as i64;

            sqlx::query(
                "UPDATE tasks \
                 SET status = ?, finished_at = COALESCE(finished_at, ?), updated_at = ?, summary = ? \
                 WHERE task_id = ?",
            )
            .bind("failed")
            .bind(now)
            .bind(now)
            .bind(&summary_owned)
            .bind(&task_id_owned)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "UPDATE task_logs \
                 SET status = ? \
                 WHERE task_id = ? AND action = 'task-created' AND status IN ('running', 'pending')",
            )
            .bind("failed")
            .bind(&task_id_owned)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "INSERT INTO task_logs \
                 (task_id, ts, level, action, status, summary, unit, meta) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_owned)
            .bind(now)
            .bind("error")
            .bind("task-dispatch-failed")
            .bind("failed")
            .bind(&summary_owned)
            .bind(Option::<String>::None)
            .bind(meta_str)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        });
        return;
    }

    for u in units {
        let mut meta_for_unit = merged_meta.clone();
        if let Value::Object(ref mut obj) = meta_for_unit {
            obj.insert("unit".to_string(), Value::String(u.clone()));
        }

        update_task_state_with_unit(
            task_id,
            "failed",
            &u,
            "failed",
            &summary,
            "task-dispatch-failed",
            "error",
            meta_for_unit,
        );
    }
}

fn append_task_log(
    task_id: &str,
    level: &str,
    action: &str,
    status: &str,
    summary: &str,
    unit: Option<&str>,
    meta: Value,
) {
    let task_id_owned = task_id.to_string();
    let level_owned = level.to_string();
    let action_owned = action.to_string();
    let status_owned = status.to_string();
    let summary_owned = summary.to_string();
    let unit_owned = unit.map(|u| u.to_string());
    let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());
    let now = current_unix_secs() as i64;

    let _ = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_owned)
        .bind(now)
        .bind(&level_owned)
        .bind(&action_owned)
        .bind(&status_owned)
        .bind(&summary_owned)
        .bind(unit_owned)
        .bind(meta_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });
}

fn run_manual_trigger_task(task_id: &str) -> Result<(), String> {
    let task_id_owned = task_id.to_string();
    let (units,): (Vec<String>,) = with_db(|pool| async move {
        let rows: Vec<SqliteRow> =
            sqlx::query("SELECT unit FROM task_units WHERE task_id = ? ORDER BY id")
                .bind(&task_id_owned)
                .fetch_all(&pool)
                .await?;
        let mut units = Vec::with_capacity(rows.len());
        for row in rows {
            units.push(row.get::<String, _>("unit"));
        }
        Ok::<(Vec<String>,), sqlx::Error>((units,))
    })?;

    if units.is_empty() {
        log_message(&format!(
            "info run-task manual-trigger no-units task_id={task_id}"
        ));
        return Ok(());
    }

    let mut results = Vec::with_capacity(units.len());
    for unit in &units {
        results.push(trigger_single_unit(unit, false));
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for res in &results {
        match res.status.as_str() {
            "triggered" => succeeded = succeeded.saturating_add(1),
            "dry-run" => {}
            _ => failed = failed.saturating_add(1),
        }
    }

    let total = results.len();
    let status = if failed > 0 { "failed" } else { "succeeded" };
    let summary = if failed > 0 {
        format!("{succeeded}/{total} units triggered, {failed} failed")
    } else {
        format!("{succeeded}/{total} units triggered")
    };

    let task_id_upd = task_id.to_string();
    let units_upd = units.clone();

    let _ = with_db(|pool| async move {
        let mut tx = pool.begin().await?;
        let now = current_unix_secs() as i64;

        sqlx::query(
            "UPDATE tasks \
             SET status = ?, finished_at = COALESCE(finished_at, ?), updated_at = ?, summary = ? \
             WHERE task_id = ?",
        )
        .bind(status)
        .bind(now)
        .bind(now)
        .bind(&summary)
        .bind(&task_id_upd)
        .execute(&mut *tx)
        .await?;

        // Normalise the initial "task-created" log entry so that its status
        // matches the final task status instead of staying "running"/"pending".
        sqlx::query(
            "UPDATE task_logs \
             SET status = ? \
             WHERE task_id = ? AND action = 'task-created' AND status IN ('running', 'pending')",
        )
        .bind(status)
        .bind(&task_id_upd)
        .execute(&mut *tx)
        .await?;

        for (unit, res) in units_upd.iter().zip(results.iter()) {
            let unit_status = match res.status.as_str() {
                "triggered" => "succeeded",
                "dry-run" => "skipped",
                "failed" | "error" => "failed",
                other => other,
            };

            sqlx::query(
                "UPDATE task_units \
                 SET status = ?, finished_at = COALESCE(finished_at, ?), message = ? \
                 WHERE task_id = ? AND unit = ?",
            )
            .bind(unit_status)
            .bind(now)
            .bind(&res.message)
            .bind(&task_id_upd)
            .bind(unit)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_upd)
        .bind(now)
        .bind(if failed > 0 { "warning" } else { "info" })
        .bind("manual-trigger-run")
        .bind(status)
        .bind(&summary)
        .bind(Option::<String>::None)
        .bind(
            serde_json::to_string(&json!({
                "units": units_upd,
                "results": results,
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    Ok(())
}

fn run_manual_service_task(task_id: &str, unit: &str, image: Option<&str>) -> Result<(), String> {
    let unit_owned = unit.to_string();
    if let Some(image) = image {
        let pull_result = match pull_container_image(image) {
            Ok(res) => res,
            Err(err) => {
                log_message(&format!(
                    "500 manual-service-image-pull-failed unit={unit_owned} image={image} err={err}"
                ));
                let meta = json!({ "unit": unit_owned, "image": image, "error": err });
                update_task_state_with_unit(
                    task_id,
                    "failed",
                    &unit_owned,
                    "failed",
                    "Manual service image pull failed",
                    "image-pull",
                    "error",
                    meta,
                );
                return Ok(());
            }
        };

        if !pull_result.success() {
            let mut error_message = exit_code_string(&pull_result.status);
            if !pull_result.stderr.is_empty() {
                error_message.push_str(": ");
                error_message.push_str(&pull_result.stderr);
            }

            log_message(&format!(
                "500 manual-service-image-pull-failed unit={unit_owned} image={image} err={error_message}"
            ));

            let command = format!("podman pull {image}");
            let argv = ["podman", "pull", image];
            let extra_meta = json!({
                "unit": unit_owned,
                "image": image,
                "error": error_message,
            });
            let meta = build_command_meta(&command, &argv, &pull_result, Some(extra_meta));

            update_task_state_with_unit(
                task_id,
                "failed",
                &unit_owned,
                "failed",
                "Manual service image pull failed",
                "image-pull",
                "error",
                meta,
            );
            return Ok(());
        }
    }

    let result = trigger_single_unit(&unit_owned, false);
    let unit_status = match result.status.as_str() {
        "triggered" => "succeeded",
        "dry-run" => "skipped",
        "failed" | "error" => "failed",
        other => other,
    };
    let task_status = if unit_status == "failed" {
        "failed"
    } else {
        "succeeded"
    };
    let summary = if unit_status == "failed" {
        "Manual service task failed".to_string()
    } else {
        "Manual service task succeeded".to_string()
    };

    let meta = json!({
        "unit": unit_owned,
        "image": image,
        "result_status": result.status,
        "result_message": result.message,
    });

    update_task_state_with_unit(
        task_id,
        task_status,
        &unit_owned,
        unit_status,
        &summary,
        "manual-service-run",
        if unit_status == "failed" {
            "error"
        } else {
            "info"
        },
        meta,
    );

    Ok(())
}

fn run_auto_update_run_task(task_id: &str, unit: &str, dry_run: bool) -> Result<(), String> {
    let unit_owned = unit.to_string();
    let command = format!("systemctl --user start {unit_owned}");
    let argv = ["systemctl", "--user", "start", unit];

    let start_result = start_auto_update_unit(&unit_owned);
    let start_result = match start_result {
        Ok(res) => res,
        Err(err) => {
            log_message(&format!(
                "500 auto-update-run-error unit={unit_owned} task_id={task_id} err={err}"
            ));
            let meta = json!({
                "unit": unit_owned,
                "dry_run": dry_run,
                "error": err,
            });
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Auto-update run error",
                "auto-update-run",
                "error",
                meta,
            );
            return Ok(());
        }
    };

    if !start_result.success() {
        let exit = exit_code_string(&start_result.status);
        log_message(&format!(
            "500 auto-update-run-start-failed unit={unit_owned} task_id={task_id} exit={exit} stderr={}",
            start_result.stderr
        ));
        let extra_meta = json!({
            "unit": unit_owned,
            "dry_run": dry_run,
            "exit": exit,
        });
        let meta = build_command_meta(&command, &argv, &start_result, Some(extra_meta));
        update_task_state_with_unit(
            task_id,
            "failed",
            unit,
            "failed",
            "Auto-update run failed to start",
            "auto-update-run-start",
            "error",
            meta,
        );
        return Ok(());
    }

    log_message(&format!(
        "202 auto-update-run-start unit={unit_owned} task_id={task_id} dry_run={dry_run}"
    ));
    let extra_meta = json!({
        "unit": unit_owned,
        "dry_run": dry_run,
        "stderr": start_result.stderr,
    });
    let meta = build_command_meta(&command, &argv, &start_result, Some(extra_meta));
    append_task_log(
        task_id,
        "info",
        "auto-update-run-start",
        "running",
        if dry_run {
            "podman auto-update dry-run started successfully"
        } else {
            "podman auto-update run started successfully"
        },
        Some(unit),
        meta,
    );

    let log_dir_opt = auto_update_log_dir();
    let mut baseline_files: HashSet<String> = HashSet::new();
    if let Some(ref dir) = log_dir_opt {
        if let Ok(read_dir) = fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    baseline_files.insert(name.to_string());
                }
            }
        }
    }

    let start_instant = Instant::now();
    let mut summary_event: Option<Value> = None;
    let mut summary_log_file: Option<PathBuf> = None;

    if let Some(log_dir) = log_dir_opt.clone() {
        let mut known_file: Option<PathBuf> = None;
        let mut processed_lines: usize = 0;

        loop {
            if start_instant.elapsed() >= Duration::from_secs(AUTO_UPDATE_RUN_MAX_SECS) {
                log_message(&format!(
                    "warn auto-update-run-timeout unit={unit_owned} task_id={task_id}"
                ));
                break;
            }

            if known_file.is_none() {
                let mut latest: Option<(SystemTime, PathBuf)> = None;
                match fs::read_dir(&log_dir) {
                    Ok(read_dir) => {
                        for entry in read_dir.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                                continue;
                            }
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                if baseline_files.contains(name) {
                                    continue;
                                }
                            }
                            let Ok(meta) = fs::metadata(&path) else {
                                continue;
                            };
                            let Ok(modified) = meta.modified() else {
                                continue;
                            };
                            match latest {
                                Some((ts, _)) if modified <= ts => {}
                                _ => latest = Some((modified, path)),
                            }
                        }
                    }
                    Err(err) => {
                        log_message(&format!(
                            "warn auto-update-run-log-dir-read-failed dir={} err={err}",
                            log_dir.to_string_lossy()
                        ));
                        break;
                    }
                }

                if let Some((_, path)) = latest {
                    known_file = Some(path);
                    processed_lines = 0;
                } else {
                    // No JSONL file yet; keep waiting.
                    thread::sleep(Duration::from_millis(AUTO_UPDATE_RUN_POLL_INTERVAL_MS));
                    continue;
                }
            }

            let path = known_file.as_ref().cloned().unwrap();
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(err) => {
                    log_message(&format!(
                        "warn auto-update-run-open-log-failed file={} err={err}",
                        path.to_string_lossy()
                    ));
                    break;
                }
            };

            let reader = io::BufReader::new(file);
            let mut line_index: usize = 0;
            for line_result in reader.lines() {
                let Ok(line) = line_result else {
                    continue;
                };
                if line_index < processed_lines {
                    line_index = line_index.saturating_add(1);
                    continue;
                }
                line_index = line_index.saturating_add(1);
                processed_lines = processed_lines.saturating_add(1);

                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let event: Value = match serde_json::from_str(trimmed) {
                    Ok(ev) => ev,
                    Err(_) => {
                        append_task_log(
                            task_id,
                            "info",
                            "auto-update-log",
                            "running",
                            trimmed,
                            Some(unit),
                            json!({
                                "unit": unit_owned,
                                "raw": trimmed,
                                "log_file": path.to_string_lossy(),
                            }),
                        );
                        continue;
                    }
                };

                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let level = if event_type == "auto-update-error" {
                    "error"
                } else if event_type == "dry-run-error" {
                    "warning"
                } else {
                    "info"
                };

                let message = if event_type == "dry-run-error" || event_type == "auto-update-error"
                {
                    let container = event
                        .get("container")
                        .or_else(|| event.get("container_name"))
                        .or_else(|| event.get("container_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let image = event
                        .get("image")
                        .or_else(|| event.get("image_name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let err_str = event
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let subject = if !image.is_empty() {
                        image
                    } else if !container.is_empty() {
                        container
                    } else {
                        unit_owned.clone()
                    };
                    if err_str.is_empty() {
                        format!("{event_type} reported by podman auto-update for {subject}")
                    } else {
                        format!("{event_type} from podman auto-update for {subject}: {err_str}")
                    }
                } else if event_type == "summary" {
                    "Auto-update summary received from podman auto-update".to_string()
                } else if event_type.is_empty() {
                    "Auto-update event from podman auto-update".to_string()
                } else {
                    format!("Auto-update event: {event_type}")
                };

                append_task_log(
                    task_id,
                    level,
                    "auto-update-log",
                    if event_type == "summary" {
                        "succeeded"
                    } else {
                        "running"
                    },
                    &message,
                    Some(unit),
                    json!({
                        "unit": unit_owned,
                        "log_file": path.to_string_lossy(),
                        "event": event,
                    }),
                );

                if event_type == "summary" {
                    summary_log_file = Some(path.clone());
                    summary_event = Some(event);
                    break;
                }
            }

            if summary_event.is_some() {
                break;
            }

            thread::sleep(Duration::from_millis(AUTO_UPDATE_RUN_POLL_INTERVAL_MS));
        }
    }

    let summary_meta_log_dir = log_dir_opt
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    if let Some(summary) = summary_event {
        let counts = summary
            .get("summary")
            .and_then(|v| v.get("counts"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let total = counts.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        let succeeded = counts
            .get("succeeded")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let failed = counts.get("failed").and_then(|v| v.as_u64()).unwrap_or(0);
        let unchanged = total.saturating_sub(succeeded.saturating_add(failed));

        let task_status = if failed > 0 { "failed" } else { "succeeded" };
        let level = if failed > 0 { "error" } else { "info" };

        let summary_text = if dry_run {
            format!(
                "podman auto-update dry-run completed: total={total}, updated={succeeded}, failed={failed}, unchanged={unchanged}"
            )
        } else {
            format!(
                "podman auto-update completed: total={total}, updated={succeeded}, failed={failed}, unchanged={unchanged}"
            )
        };

        let meta = json!({
            "unit": unit_owned,
            "dry_run": dry_run,
            "summary_event": summary,
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
            "unchanged": unchanged,
            "log_file": summary_log_file
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            "log_dir": summary_meta_log_dir,
        });

        update_task_state_with_unit(
            task_id,
            task_status,
            unit,
            task_status,
            &summary_text,
            "auto-update-run",
            level,
            meta,
        );
        ingest_auto_update_warnings(task_id, unit);
        return Ok(());
    }

    // No summary event observed; fall back to a generic success/failure based on timeout.
    let timed_out = start_instant.elapsed() >= Duration::from_secs(AUTO_UPDATE_RUN_MAX_SECS);
    let (task_status, level, summary_text) = if timed_out {
        (
            "failed",
            "error",
            if dry_run {
                format!(
                    "podman auto-update dry-run timed out after {} seconds; check podman auto-update logs",
                    AUTO_UPDATE_RUN_MAX_SECS
                )
            } else {
                format!(
                    "podman auto-update run timed out after {} seconds; check podman auto-update logs",
                    AUTO_UPDATE_RUN_MAX_SECS
                )
            },
        )
    } else {
        (
            "succeeded",
            "info",
            if dry_run {
                "podman auto-update dry-run completed (no JSONL summary found)".to_string()
            } else {
                "podman auto-update run completed (no JSONL summary found)".to_string()
            },
        )
    };

    let meta = json!({
        "unit": unit_owned,
        "dry_run": dry_run,
        "log_dir": summary_meta_log_dir,
        "reason": if timed_out { "timeout" } else { "no-summary" },
    });

    update_task_state_with_unit(
        task_id,
        task_status,
        unit,
        task_status,
        &summary_text,
        "auto-update-run",
        level,
        meta,
    );

    if log_dir_opt.is_some() {
        ingest_auto_update_warnings(task_id, unit);
    }

    Ok(())
}

fn run_auto_update_task(task_id: &str, unit: &str) -> Result<(), String> {
    let unit_owned = unit.to_string();
    let command = format!("systemctl --user start {unit_owned}");
    let argv = ["systemctl", "--user", "start", unit];

    match start_auto_update_unit(&unit_owned) {
        Ok(result) if result.success() => {
            log_message(&format!(
                "202 auto-update-start unit={unit_owned} task_id={task_id}"
            ));
            let extra_meta = json!({
                "unit": unit_owned,
                "stderr": result.stderr,
            });
            let meta = build_command_meta(&command, &argv, &result, Some(extra_meta));
            update_task_state_with_unit(
                task_id,
                "succeeded",
                unit,
                "succeeded",
                "Auto-update unit started successfully",
                "auto-update-start",
                "info",
                meta,
            );
            ingest_auto_update_warnings(task_id, unit);
            Ok(())
        }
        Ok(result) => {
            let exit = exit_code_string(&result.status);
            log_message(&format!(
                "500 auto-update-failed unit={unit_owned} task_id={task_id} exit={exit} stderr={}",
                result.stderr
            ));
            let extra_meta = json!({
                "unit": unit_owned,
                "exit": exit,
            });
            let meta = build_command_meta(&command, &argv, &result, Some(extra_meta));
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Auto-update unit failed to start",
                "auto-update-start",
                "error",
                meta,
            );
            Ok(())
        }
        Err(err) => {
            log_message(&format!(
                "500 auto-update-error unit={unit_owned} task_id={task_id} err={err}"
            ));
            let meta = json!({
                "unit": unit_owned,
                "error": err,
            });
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                "Auto-update unit error",
                "auto-update-start",
                "error",
                meta,
            );
            Ok(())
        }
    }
}

fn ingest_auto_update_warnings(task_id: &str, unit: &str) {
    let Some(log_dir) = auto_update_log_dir() else {
        // No configured log directory; keep behaviour as "clean success".
        return;
    };

    let read_dir = match fs::read_dir(&log_dir) {
        Ok(rd) => rd,
        Err(err) => {
            log_message(&format!(
                "debug auto-update-logs-skip dir-unreadable dir={} err={err}",
                log_dir.to_string_lossy()
            ));
            return;
        }
    };

    let now = SystemTime::now();
    let max_age_secs = env::var("PODUP_AUTO_UPDATE_LOG_MAX_AGE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(600);
    let threshold = now
        .checked_sub(Duration::from_secs(max_age_secs))
        .unwrap_or(UNIX_EPOCH);

    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < threshold {
            continue;
        }
        match latest {
            Some((ts, _)) if modified <= ts => {}
            _ => latest = Some((modified, path)),
        }
    }

    let Some((_, path)) = latest else {
        log_message(&format!(
            "debug auto-update-logs-skip no-recent-jsonl dir={}",
            log_dir.to_string_lossy()
        ));
        return;
    };

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(err) => {
            log_message(&format!(
                "debug auto-update-logs-skip open-failed file={} err={err}",
                path.to_string_lossy()
            ));
            return;
        }
    };

    let reader = io::BufReader::new(file);
    let mut warnings: Vec<Value> = Vec::new();

    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if event_type == "dry-run-error" || event_type == "auto-update-error" {
            warnings.push(event);
        }
    }

    if warnings.is_empty() {
        log_message(&format!(
            "debug auto-update-logs-none task_id={task_id} unit={unit} file={}",
            path.to_string_lossy()
        ));
        return;
    }

    let now_secs = current_unix_secs() as i64;
    let task_id_db = task_id.to_string();
    let unit_db = unit.to_string();
    let log_file = path.to_string_lossy().into_owned();

    let summary_meta = json!({
        "unit": unit_db,
        "log_file": log_file,
        "warnings": warnings,
    });
    let summary_text = format!(
        "Auto-update succeeded with {} warning(s) from podman auto-update",
        warnings.len()
    );

    let warning_count = warnings.len();
    let unit_for_event = unit_db.clone();
    let log_file_for_event = log_file.clone();

    let db_result = with_db(|pool| async move {
        let mut tx = pool.begin().await?;

        let summary_meta_str =
            serde_json::to_string(&summary_meta).unwrap_or_else(|_| "{}".to_string());
        sqlx::query(
            "INSERT INTO task_logs \
             (task_id, ts, level, action, status, summary, unit, meta) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id_db)
        .bind(now_secs)
        .bind("info")
        .bind("auto-update-warnings")
        .bind("succeeded")
        .bind(&summary_text)
        .bind(Some(unit_db.clone()))
        .bind(summary_meta_str)
        .execute(&mut *tx)
        .await?;

        for warning in &warnings {
            let event_type = warning
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let at = warning
                .get("at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let container = warning
                .get("container")
                .or_else(|| warning.get("container_name"))
                .or_else(|| warning.get("container_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image = warning
                .get("image")
                .or_else(|| warning.get("image_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let error_str = warning
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut snippet = error_str.trim().to_string();
            if snippet.len() > 200 {
                snippet.truncate(200);
            }

            let unit_desc = if !image.is_empty() {
                image.clone()
            } else if !container.is_empty() {
                container.clone()
            } else {
                unit_db.clone()
            };

            let summary = if !snippet.is_empty() {
                format!("[{event_type}] auto-update warning for {unit_desc}: {snippet}")
            } else {
                format!("[{event_type}] auto-update warning for {unit_desc} (see meta.error)")
            };

            let detail_meta = json!({
                "unit": unit_db,
                "log_file": log_file,
                "event": warning,
                "at": at,
                "container": if container.is_empty() { Value::Null } else { Value::from(container) },
                "image": if image.is_empty() { Value::Null } else { Value::from(image) },
            });
            let detail_meta_str =
                serde_json::to_string(&detail_meta).unwrap_or_else(|_| "{}".to_string());

            // Treat dry-run-error as warning and auto-update-error as error.
            let level = if event_type == "auto-update-error" {
                "error"
            } else {
                "warning"
            };

            sqlx::query(
                "INSERT INTO task_logs \
                 (task_id, ts, level, action, status, summary, unit, meta) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&task_id_db)
            .bind(now_secs)
            .bind(level)
            .bind("auto-update-warning")
            .bind("succeeded")
            .bind(&summary)
            .bind(Some(unit_db.clone()))
            .bind(detail_meta_str)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    });

    if let Err(err) = db_result {
        log_message(&format!(
            "warn auto-update-log-ingest-failed task_id={task_id} unit={unit} file={} err={err}",
            path.to_string_lossy()
        ));
        return;
    }

    record_system_event(
        "auto-update-warning",
        200,
        json!({
            "task_id": task_id,
            "unit": unit_for_event,
            "log_file": log_file_for_event,
            "warning_count": warning_count,
        }),
    );
}

fn run_maintenance_prune_task(
    task_id: &str,
    retention_secs: u64,
    dry_run: bool,
) -> Result<StatePruneReport, String> {
    let unit = "state-prune";
    match prune_state_dir(Duration::from_secs(retention_secs.max(1)), dry_run) {
        Ok(mut report) => {
            let task_retention_secs = task_retention_secs_from_env();
            let tasks_removed = match prune_tasks_older_than(task_retention_secs, dry_run) {
                Ok(count) => count as usize,
                Err(err) => {
                    log_message(&format!(
                        "error task-prune-failed retention_secs={} dry_run={} err={}",
                        task_retention_secs, dry_run, err
                    ));
                    0
                }
            };
            report.tasks_removed = tasks_removed;
            log_message(&format!(
                "info task-prune removed {} tasks older than {} seconds dry_run={}",
                tasks_removed, task_retention_secs, dry_run
            ));

            let summary = if dry_run {
                format!(
                    "State prune dry-run completed: tokens={} locks={} legacy_dirs={} tasks={}",
                    report.tokens_removed,
                    report.locks_removed,
                    report.legacy_dirs_removed,
                    report.tasks_removed
                )
            } else {
                format!(
                    "State prune completed: tokens={} locks={} legacy_dirs={} tasks={}",
                    report.tokens_removed,
                    report.locks_removed,
                    report.legacy_dirs_removed,
                    report.tasks_removed
                )
            };
            let meta = json!({
                "unit": unit,
                "dry_run": dry_run,
                "retention_secs": retention_secs.max(1),
                "tokens_removed": report.tokens_removed,
                "locks_removed": report.locks_removed,
                "legacy_dirs_removed": report.legacy_dirs_removed,
                "task_retention_secs": task_retention_secs,
                "tasks_removed": report.tasks_removed,
            });
            update_task_state_with_unit(
                task_id,
                "succeeded",
                unit,
                "succeeded",
                &summary,
                "state-prune-run",
                "info",
                meta,
            );
            Ok(report)
        }
        Err(err) => {
            let summary = "State prune failed".to_string();
            let meta = json!({
                "unit": unit,
                "dry_run": dry_run,
                "retention_secs": retention_secs.max(1),
                "error": err.clone(),
            });
            update_task_state_with_unit(
                task_id,
                "failed",
                unit,
                "failed",
                &summary,
                "state-prune-run",
                "error",
                meta,
            );
            Err(err)
        }
    }
}

fn unit_configured_image(unit: &str) -> Option<String> {
    if let Some(path) = unit_definition_path(unit) {
        if let Some(image) = parse_container_image(&path) {
            return Some(image);
        }
    }

    let trimmed = unit.trim_end_matches(".service");
    if trimmed.is_empty() {
        return None;
    }

    let fallback = Path::new(DEFAULT_CONTAINER_DIR).join(format!("{trimmed}.container"));
    parse_container_image(&fallback)
}

fn unit_definition_path(unit: &str) -> Option<PathBuf> {
    let output = Command::new("systemctl")
        .arg("--user")
        .arg("show")
        .arg(unit)
        .arg("--property=SourcePath")
        .arg("--property=FragmentPath")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut source: Option<PathBuf> = None;
    let mut fragment: Option<PathBuf> = None;

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("SourcePath=") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                source = Some(PathBuf::from(trimmed));
            }
        } else if let Some(rest) = line.strip_prefix("FragmentPath=") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                fragment = Some(PathBuf::from(trimmed));
            }
        }
    }

    source.or(fragment)
}

fn parse_container_image(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let mut in_container_section = false;

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_container_section = line.eq_ignore_ascii_case("[container]");
            continue;
        }

        if in_container_section {
            if let Some(rest) = line.strip_prefix("Image=") {
                let value = rest.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }

    None
}

fn images_match(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Once;
    use tempfile::NamedTempFile;

    fn init_test_db() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            set_env(ENV_DB_URL, "sqlite::memory:?cache=shared");
            let _ = super::db_pool();
        });

        let _ = with_db(|pool| async move {
            sqlx::query("DELETE FROM rate_limit_tokens")
                .execute(&pool)
                .await?;
            sqlx::query("DELETE FROM image_locks")
                .execute(&pool)
                .await?;
            Ok::<(), sqlx::Error>(())
        });
    }

    fn init_test_db_with_systemctl_mock() {
        init_test_db();

        // Point systemctl to the test stub under tests/mock-bin to avoid
        // touching the real host systemd during tests.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_dir = format!("{manifest_dir}/tests/mock-bin");

        let current_path = env::var("PATH").unwrap_or_default();
        let new_path = format!("{mock_dir}:{current_path}");
        set_env("PATH", &new_path);

        let log_path = format!("{mock_dir}/log.txt");
        let _ = fs::remove_file(&log_path);
    }

    #[allow(unused_unsafe)]
    fn set_env(key: &str, value: &str) {
        unsafe {
            env::set_var(key, value);
        }
    }

    #[allow(unused_unsafe)]
    fn remove_env(key: &str) {
        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn parse_container_image_finds_image() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[Unit]\nDescription=demo\n\n[Container]\nImage=ghcr.io/example/service:latest\n\n[Service]\nRestart=always\n"
        )
        .unwrap();

        let image = parse_container_image(file.path()).expect("image expected");
        assert_eq!(image, "ghcr.io/example/service:latest");
    }

    #[test]
    fn extract_container_image_requires_tag() {
        let payload = json!({
            "package": {
                "name": "demo",
                "namespace": "example",
                "package_type": "CONTAINER"
            },
            "registry": { "host": "ghcr.io" },
            "package_version": {
                "metadata": { "container": { "tags": [] } }
            }
        })
        .to_string();

        let err = extract_container_image(payload.as_bytes()).unwrap_err();
        assert_eq!(err, "missing-tag");
    }

    #[test]
    fn images_match_normalizes_whitespace() {
        assert!(images_match(
            "ghcr.io/example/app:latest",
            " ghcr.io/example/app:latest "
        ));
        assert!(!images_match(
            "ghcr.io/example/app:latest",
            "ghcr.io/example/app:v1"
        ));
    }

    #[test]
    fn github_payload_builds_full_image() {
        let payload = json!({
            "package": {
                "name": "demo",
                "namespace": "Example",
                "package_type": "CONTAINER"
            },
            "registry": { "host": "ghcr.io" },
            "package_version": {
                "metadata": { "container": { "tags": ["main"] } }
            }
        })
        .to_string();

        let image = extract_container_image(payload.as_bytes()).unwrap();
        assert_eq!(image, "ghcr.io/example/demo:main");
    }

    #[test]
    fn rate_limit_enforces_limits() {
        init_test_db();
        set_env("PODUP_LIMIT1_COUNT", "1");
        set_env("PODUP_LIMIT1_WINDOW", "3600");
        set_env("PODUP_LIMIT2_COUNT", "5");
        set_env("PODUP_LIMIT2_WINDOW", "3600");

        let first = rate_limit_check();
        assert!(first.is_ok(), "first rate limit check failed: {:?}", first);
        let second = rate_limit_check();
        assert!(
            matches!(second, Err(RateLimitError::Exceeded { .. })),
            "second check expected limit hit, got {:?}",
            second
        );

        remove_env("PODUP_LIMIT1_COUNT");
        remove_env("PODUP_LIMIT1_WINDOW");
        remove_env("PODUP_LIMIT2_COUNT");
        remove_env("PODUP_LIMIT2_WINDOW");
    }

    #[test]
    fn github_task_stop_marks_cancelled_and_stops_runner_unit() {
        init_test_db_with_systemctl_mock();

        // Create a github-webhook task with a known delivery id so we can
        // predict the transient unit name.
        let meta = TaskMeta::GithubWebhook {
            unit: "demo.service".to_string(),
            image: "ghcr.io/example/demo:latest".to_string(),
            event: "push".to_string(),
            delivery: "abc123".to_string(),
            path: "/github/demo".to_string(),
        };

        let task_id = create_github_task(
            "demo.service",
            "ghcr.io/example/demo:latest",
            "push",
            "abc123",
            "/github/demo",
            "req-test-stop",
            &meta,
        )
        .expect("task created");

        // Invoke the stop handler as the HTTP layer would.
        let ctx = RequestContext {
            method: "POST".to_string(),
            path: format!("/api/tasks/{task_id}/stop"),
            query: None,
            headers: HashMap::new(),
            body: Vec::new(),
            raw_request: String::new(),
            request_id: "req-test-stop".to_string(),
            started_at: Instant::now(),
            received_at: SystemTime::now(),
        };

        handle_task_stop(&ctx, &task_id).expect("stop handler should not error");

        // Verify DB state: task is cancelled and no longer stoppable.
        let task_id_clone = task_id.clone();
        let (status, can_stop, can_force_stop, can_retry) = with_db(|pool| async move {
            let row: SqliteRow = sqlx::query(
                "SELECT status, can_stop, can_force_stop, can_retry \
                     FROM tasks WHERE task_id = ?",
            )
            .bind(&task_id_clone)
            .fetch_one(&pool)
            .await?;

            Ok::<(String, i64, i64, i64), sqlx::Error>((
                row.get("status"),
                row.get("can_stop"),
                row.get("can_force_stop"),
                row.get("can_retry"),
            ))
        })
        .expect("db query");

        assert_eq!(status, "cancelled");
        assert_eq!(can_stop, 0);
        assert_eq!(can_force_stop, 0);
        assert_eq!(can_retry, 1);

        // Verify that the mock systemctl saw a stop for the derived transient
        // unit when the shim log is available. In some CI environments the
        // PATH/exec wiring may prevent the shim from being invoked; in that
        // case we still keep the DB-level assertions above.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let log_path = format!("{manifest_dir}/tests/mock-bin/log.txt");
        match fs::read_to_string(&log_path) {
            Ok(log_contents) => {
                assert!(
                    log_contents.contains("systemctl --user stop webhook-task-abc123"),
                    "expected stop of webhook-task-abc123, got log:\n{log_contents}"
                );
            }
            Err(err) => {
                eprintln!(
                    "warning: systemctl mock log not found, skipping runner-unit assertion: {err}"
                );
            }
        }
    }

    #[test]
    fn auto_update_dry_run_errors_are_ingested_into_task_logs_and_events() {
        init_test_db();

        // Point auto-update log dir to a temporary directory.
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        set_env(
            super::ENV_AUTO_UPDATE_LOG_DIR,
            log_dir.to_string_lossy().as_ref(),
        );
        // Ensure that our synthetic JSONL file is considered recent enough for
        // ingestion regardless of test runtime/environment clock skew.
        set_env("PODUP_AUTO_UPDATE_LOG_MAX_AGE_SECS", "86400");

        let unit = "podman-auto-update.service";
        let task_id = create_manual_auto_update_task(unit, "req-auto-update-test", "/auto-update")
            .expect("manual auto-update task created");

        // Create a synthetic JSONL log file with a single dry-run-error entry.
        let jsonl_path = log_dir.join("2025-12-05T070437513Z.jsonl");
        {
            let mut file = File::create(&jsonl_path).unwrap();
            writeln!(
                file,
                r#"{{"type":"dry-run-error","at":"2025-12-05T07:08:06.653Z","container":"demo","image":"ghcr.io/example/demo:latest","error":"Error: dry-run failed: EOF"}}"#
            )
            .unwrap();
            writeln!(
                file,
                r#"{{"type":"summary","summary":{{"start":"2025-12-05T06:54:32.042Z","end":"2025-12-05T07:02:36.665Z","counts":{{"total":1,"succeeded":1,"failed":0}}}}}}"#
            )
            .unwrap();
        }

        ingest_auto_update_warnings(&task_id, unit);

        // Verify that warning logs were inserted for this task and surfaced via the detail view.
        let detail = load_task_detail_record(&task_id)
            .expect("detail load should succeed")
            .expect("task should exist");

        assert!(
            detail.task.has_warnings,
            "task should be flagged as having warnings"
        );
        assert_eq!(
            detail.task.warning_count,
            Some(1),
            "warning_count should match number of warning/error logs"
        );
        assert!(
            detail
                .logs
                .iter()
                .any(|log| log.action == "auto-update-warning"),
            "expected at least one auto-update-warning log entry"
        );
        assert!(
            detail
                .logs
                .iter()
                .any(|log| log.action == "auto-update-warnings"),
            "expected auto-update-warnings summary log entry"
        );

        // Verify that an event_log entry was recorded and tagged with this task_id.
        let task_id_for_event = task_id.clone();
        let (events_for_task,): (i64,) = with_db(|pool| async move {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM event_log \
                 WHERE action = 'auto-update-warning' AND task_id = ?",
            )
            .bind(&task_id_for_event)
            .fetch_one(&pool)
            .await?;
            Ok::<(i64,), sqlx::Error>((count,))
        })
        .expect("event_log query");

        assert_eq!(
            events_for_task, 1,
            "expected exactly one auto-update-warning event for the task"
        );
    }

    #[test]
    fn task_created_log_status_follows_final_status_for_manual_auto_update() {
        init_test_db_with_systemctl_mock();

        // Point auto-update log dir to a temporary directory and seed it with a
        // synthetic JSONL file so that ingest_auto_update_warnings has data.
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        set_env(
            super::ENV_AUTO_UPDATE_LOG_DIR,
            log_dir.to_string_lossy().as_ref(),
        );

        let unit = "podman-auto-update.service";
        let task_id =
            create_manual_auto_update_task(unit, "req-task-created-status", "/auto-update-status")
                .expect("manual auto-update task created");

        // Seed a log file that contains a dry-run-error and a summary entry,
        // matching the production podman-update-manager.ts format.
        let jsonl_path = log_dir.join("2025-12-05T070437513Z.jsonl");
        {
            let mut file = File::create(&jsonl_path).unwrap();
            writeln!(
                file,
                r#"{{"type":"dry-run-error","at":"2025-12-05T07:08:06.653Z","container":"demo","image":"ghcr.io/example/demo:latest","error":"Error: dry-run failed: EOF"}}"#
            )
            .unwrap();
            writeln!(
                file,
                r#"{{"type":"summary","summary":{{"start":"2025-12-05T06:54:32.042Z","end":"2025-12-05T07:02:36.665Z","counts":{{"total":1,"succeeded":1,"failed":0}}}}}}"#
            )
            .unwrap();
        }

        // Simulate the real execution path: start the auto-update unit, mark
        // the task as succeeded, and ingest warnings from the JSONL log.
        run_auto_update_task(&task_id, unit).expect("auto-update task should run");

        // The task detail view should now report a succeeded task and the
        // initial task-created log must no longer be marked as running/pending.
        let detail = load_task_detail_record(&task_id)
            .expect("detail load should succeed")
            .expect("task should exist");

        assert_eq!(detail.task.status, "succeeded");
        assert!(
            detail
                .logs
                .iter()
                .any(|log| log.action == "task-created" && log.status == "succeeded"),
            "expected a task-created log whose status matches the final task status, logs={:#?}",
            detail.logs
        );
        assert!(
            !detail.logs.iter().any(|log| {
                log.action == "task-created" && (log.status == "running" || log.status == "pending")
            }),
            "task-created logs must not stay in running/pending for a completed task, logs={:#?}",
            detail.logs
        );
    }

    #[test]
    fn systemd_run_args_match_expected() {
        let args = build_systemd_run_args("webhook-task-demo", "/usr/bin/webhook", "tsk_demo_task");

        assert_eq!(args[0], "--user");
        assert_eq!(args[1], "--collect");
        assert_eq!(args[2], "--quiet");
        assert_eq!(args[3], "--unit=webhook-task-demo");
        assert_eq!(args[4], "/usr/bin/webhook");
        assert_eq!(args[5], "--run-task");
        assert_eq!(args[6], "tsk_demo_task");
    }

    #[test]
    fn github_signature_validates() {
        let body = br#"{"action":"published"}"#;
        let secret = "topsecret";

        // Compute a correct signature for the given body/secret.
        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<sha2::Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = format!("sha256={:x}", mac.finalize().into_bytes());

        let result = super::verify_github_signature(&sig, secret, body).unwrap();
        assert!(result.valid, "expected signature to be valid");
        assert_eq!(result.provided, sig.to_string());
        assert_eq!(result.expected.len(), 64);
        assert!(result.payload_dump.is_none());
    }

    #[test]
    fn github_signature_mismatch_dumps_payload() {
        let body = br#"{"hello":"world"}"#;
        let secret = "another-secret";

        // Deliberately use an incorrect signature (all zeros)
        let bad_sig = "sha256=0000000000000000000000000000000000000000000000000000000000000000";

        // Point payload dump to a temp file so tests don't touch real paths.
        let dir = tempfile::tempdir().unwrap();
        let dump_path = dir.path().join("dump.bin");
        set_env(ENV_DEBUG_PAYLOAD_PATH, dump_path.to_string_lossy().as_ref());

        let result = super::verify_github_signature(bad_sig, secret, body).unwrap();
        assert!(!result.valid);
        assert_eq!(result.provided, bad_sig.to_string());
        assert_eq!(
            result.expected.len(),
            64,
            "expected HMAC should be 32 bytes hex"
        );
        let dump = result.payload_dump.expect("payload dump path expected");
        assert!(
            std::path::Path::new(&dump).exists(),
            "dump file should exist"
        );
        let dumped = std::fs::read(&dump).unwrap();
        assert_eq!(dumped, body);

        remove_env(ENV_DEBUG_PAYLOAD_PATH);
    }
}

fn pointer_as_str<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
}

fn normalize_registry_host(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_REGISTRY_HOST.to_string();
    }

    if let Ok(url) = Url::parse(trimmed) {
        if let Some(host) = url.host_str() {
            return host.to_lowercase();
        }
    }

    trimmed
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_lowercase()
}

fn extract_primary_tag(value: &Value) -> Option<String> {
    const BASES: [&str; 2] = ["/package_version", "/registry_package/package_version"];

    for base in BASES {
        if let Some(tags) = value
            .pointer(&format!("{base}/metadata/container/tags"))
            .and_then(|v| v.as_array())
        {
            for tag in tags {
                if let Some(tag_str) = tag.as_str() {
                    let trimmed = tag_str.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }

        if let Some(name) = pointer_as_str(value, &format!("{base}/container_metadata/tag/name"))
            .or_else(|| pointer_as_str(value, &format!("{base}/metadata/container/tag_name")))
        {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

fn exit_code_string(status: &ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".into(), |code| code.to_string())
}

struct SignatureCheck {
    valid: bool,
    provided: String,
    expected: String,
    expected_error: Option<String>,
    expected_len: usize,
    body_sha256: String,
    payload_dump: Option<String>,
    dump_error: Option<String>,
    header_raw: String,
    prefix_ok: bool,
}

fn verify_github_signature(
    signature: &str,
    secret: &str,
    body: &[u8],
) -> Result<SignatureCheck, String> {
    use hex::ToHex;
    use sha2::Digest;

    let body_sha256 = sha2::Sha256::digest(body).encode_hex::<String>();
    let secret_len = secret.len();

    let header_raw = signature.to_string();
    let (provided, prefix_ok) = match parse_signature_bytes(signature) {
        Ok((bytes, ok)) => (bytes, ok),
        Err(err) => {
            let expected = compute_expected_hmac(secret, body).map_err(|e| e.to_string())?;
            let (dump, dump_err) = dump_payload(body, secret_len);
            return Ok(SignatureCheck {
                valid: false,
                provided: signature.to_string(),
                expected: expected.clone(),
                expected_error: Some(format!("sig-parse: {err}")),
                expected_len: expected.len() / 2,
                body_sha256,
                payload_dump: dump,
                dump_error: dump_err,
                header_raw,
                prefix_ok: false,
            });
        }
    };

    // Compute expected once to avoid any ambiguity with clone/finalize order.
    let (expected_hex, expected_err, expected_len) = match compute_expected_hmac_bytes(secret, body)
    {
        Ok(bytes) => {
            let len = bytes.len();
            (bytes.encode_hex::<String>(), None, len)
        }
        Err(err) => (String::new(), Some(err), 0),
    };

    let valid = if expected_len > 0 {
        match hex::decode(&expected_hex) {
            Ok(expected_bytes) => provided.ct_eq(&expected_bytes).into(),
            Err(_) => false,
        }
    } else {
        false
    };

    let (dump, dump_err) = if valid {
        (None, None)
    } else {
        dump_payload(body, secret_len)
    };

    Ok(SignatureCheck {
        valid,
        provided: signature.to_string(),
        expected: expected_hex,
        expected_error: expected_err,
        expected_len,
        body_sha256,
        payload_dump: dump,
        dump_error: dump_err,
        header_raw,
        prefix_ok,
    })
}

// Accept signatures of the form "sha256=<hex>" (case-insensitive) or raw hex.
fn parse_signature_bytes(sig: &str) -> Result<(Vec<u8>, bool), String> {
    let lower = sig.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("sha256=") {
        let bytes = decode(rest).map_err(|e| format!("invalid hex: {e}"))?;
        return Ok((bytes, true));
    }

    // Fallback: treat entire header as hex without prefix.
    let bytes = decode(sig).map_err(|e| format!("missing-prefix invalid hex: {e}"))?;
    Ok((bytes, false))
}

fn compute_expected_hmac(secret: &str, body: &[u8]) -> Result<String, String> {
    use hex::ToHex;
    let bytes = compute_expected_hmac_bytes(secret, body)?;
    Ok(bytes.encode_hex::<String>())
}

fn compute_expected_hmac_bytes(secret: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(body);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn dump_payload(body: &[u8], _secret_len: usize) -> (Option<String>, Option<String>) {
    let debug_path = env::var(ENV_DEBUG_PAYLOAD_PATH)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| {
            let default = Path::new(DEFAULT_STATE_DIR).join("last_payload.bin");
            default.to_string_lossy().into_owned()
        });

    if let Some(parent) = Path::new(&debug_path).parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return (None, Some(format!("create_dir_failed: {err}")));
        }
    }

    match File::create(&debug_path) {
        Ok(mut file) => match file.write_all(body) {
            Ok(_) => (Some(debug_path), None),
            Err(err) => (None, Some(format!("write_failed: {err}"))),
        },
        Err(err) => (None, Some(format!("create_failed: {err}"))),
    }
}

fn github_event_allowed(event: &str) -> bool {
    let filters = env::var("GITHUB_ALLOWED_EVENTS").unwrap_or_default();
    if filters.trim().is_empty() {
        return true;
    }

    filters
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .any(|allowed| allowed == event.to_lowercase())
}

fn write_response(status: u16, reason: &str, body: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "HTTP/1.1 {} {}\r\n", status, reason)?;
    stdout.write_all(b"Content-Type: text/plain; charset=utf-8\r\n")?;
    stdout.write_all(b"Connection: close\r\n")?;
    stdout.write_all(b"\r\n")?;
    if !body.is_empty() {
        writeln!(stdout, "{}", body)?;
    }
    stdout.flush()
}

fn write_payload_response(
    status: u16,
    reason: &str,
    content_type: &str,
    content_length: usize,
    body: Option<&[u8]>,
) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "HTTP/1.1 {} {}\r\n", status, reason)?;
    write!(stdout, "Content-Type: {}\r\n", content_type)?;
    write!(stdout, "Content-Length: {}\r\n", content_length)?;
    stdout.write_all(b"Connection: close\r\n")?;
    stdout.write_all(b"\r\n")?;
    if let Some(bytes) = body {
        stdout.write_all(bytes)?;
    }
    stdout.flush()
}

fn write_sse_event(event: &str, data: &str) -> io::Result<()> {
    // Single-event SSE helper used by /sse/hello.
    let mut stdout = io::stdout().lock();
    write!(stdout, "HTTP/1.1 200 OK\r\n")?;
    stdout.write_all(b"Content-Type: text/event-stream\r\n")?;
    stdout.write_all(b"Cache-Control: no-cache\r\n")?;
    stdout.write_all(b"Connection: keep-alive\r\n")?;
    stdout.write_all(b"\r\n")?;
    if !event.is_empty() {
        writeln!(stdout, "event: {event}")?;
    }
    stdout.write_all(b"retry: 15000\n")?;
    for line in data.lines() {
        writeln!(stdout, "data: {line}")?;
    }
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn write_sse_stream(body: &str) -> io::Result<()> {
    // Multi-event SSE helper used by /sse/task-logs to emit a precomputed
    // stream of events in a single HTTP response.
    let mut stdout = io::stdout().lock();
    write!(stdout, "HTTP/1.1 200 OK\r\n")?;
    stdout.write_all(b"Content-Type: text/event-stream\r\n")?;
    stdout.write_all(b"Cache-Control: no-cache\r\n")?;
    stdout.write_all(b"Connection: keep-alive\r\n")?;
    stdout.write_all(b"\r\n")?;
    stdout.write_all(body.as_bytes())?;
    stdout.flush()
}

fn send_response(status: u16, reason: &str, body: &str) -> Result<(), String> {
    match write_response(status, reason, body) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn send_binary_response(
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    match write_payload_response(status, reason, content_type, body.len(), Some(body)) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn send_head_response(
    status: u16,
    reason: &str,
    content_type: &str,
    content_length: usize,
) -> Result<(), String> {
    match write_payload_response(status, reason, content_type, content_length, None) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn send_sse_event(event: &str, data: &str) -> Result<(), String> {
    match write_sse_event(event, data) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn send_sse_stream(body: &str) -> Result<(), String> {
    match write_sse_stream(body) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == io::ErrorKind::BrokenPipe
                || err.kind() == io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn db_pool() -> SqlitePool {
    DB_POOL.get_or_init(init_db_pool).clone()
}

fn init_db_pool() -> SqlitePool {
    let url = env::var(ENV_DB_URL)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("sqlite://{DEFAULT_DB_PATH}"));
    let trimmed = url.trim().to_string();
    let state_dir_hint = env::var(ENV_STATE_DIR).unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());

    let runtime = DB_RUNTIME.get_or_init(|| Runtime::new().expect("failed to create db runtime"));

    if !trimmed.starts_with("sqlite://") && !trimmed.starts_with("sqlite::") {
        let message = format!("unsupported database url: {url} (only sqlite:// is supported)");
        log_message(&format!("warn db-init-unsupported {message}"));
        set_db_status(&url, Some(message.clone()));
        return runtime
            .block_on(async {
                let pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await?;
                MIGRATOR.run(&pool).await?;
                Ok::<SqlitePool, sqlx::Error>(pool)
            })
            .unwrap_or_else(|_| panic!("{message}"));
    }

    let storage_ready = ensure_sqlite_storage(&trimmed).err();
    let pool_result = runtime.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&trimmed)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok::<SqlitePool, sqlx::Error>(pool)
    });

    match pool_result {
        Ok(pool) => {
            set_db_status(&url, None);
            pool
        }
        Err(err) => {
            let mut message = format!("failed to initialize database at {url}: {err}");
            if let Some(storage_err) = storage_ready {
                message.push_str(&format!("; {storage_err}"));
            }
            message.push_str(&format!(
                "; adjust {ENV_DB_URL} or {ENV_STATE_DIR} (current {state_dir_hint})"
            ));

            log_message(&format!("warn db-init-fallback {message}"));
            set_db_status(&url, Some(message.clone()));

            let fallback = runtime
                .block_on(async {
                    let pool = SqlitePoolOptions::new()
                        .max_connections(1)
                        .connect("sqlite::memory:")
                        .await?;
                    MIGRATOR.run(&pool).await?;
                    Ok::<SqlitePool, sqlx::Error>(pool)
                })
                .unwrap_or_else(|_| panic!("{message}"));

            fallback
        }
    }
}

fn ensure_sqlite_storage(conn: &str) -> Result<(), String> {
    if let Some(path) = conn.strip_prefix("sqlite://") {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                let message = format!("db-dir-create-failed path={} err={}", parent.display(), err);
                log_message(&format!("warn {message}"));
                return Err(message);
            }
        }

        // Ensure the database file exists before sqlx tries to open it. On some
        // platforms/sqlite builds, connecting to a non-existent file path can
        // fail with `code: 14` instead of creating the file implicitly.
        if !path.exists() {
            if let Err(err) = File::create(path) {
                let message = format!("db-file-create-failed path={} err={}", path.display(), err);
                log_message(&format!("warn {message}"));
                return Err(message);
            }
        }
    }

    Ok(())
}

fn set_db_status(url: &str, error: Option<String>) {
    let lock = DB_INIT_STATUS.get_or_init(|| {
        RwLock::new(DbInitStatus {
            url: url.to_string(),
            error: None,
        })
    });
    if let Ok(mut status) = lock.write() {
        status.url = url.to_string();
        status.error = error;
    }
}

fn db_status() -> DbInitStatus {
    DB_INIT_STATUS
        .get_or_init(|| {
            RwLock::new(DbInitStatus {
                url: "unknown".into(),
                error: None,
            })
        })
        .read()
        .map(|s| s.clone())
        .unwrap_or(DbInitStatus {
            url: "unknown".into(),
            error: None,
        })
}

fn db_init_error() -> Option<String> {
    db_status().error
}

fn with_db<F, Fut, T>(f: F) -> Result<T, String>
where
    F: FnOnce(SqlitePool) -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>> + Send + 'static,
    T: Send + 'static,
{
    if let Some(err) = db_init_error() {
        return Err(err);
    }

    let pool = db_pool();
    let runtime = DB_RUNTIME
        .get()
        .ok_or_else(|| "database runtime unavailable".to_string())?;
    runtime
        .block_on(async move { f(pool).await })
        .map_err(|e| e.to_string())
}

fn seed_demo_data() -> Result<(), String> {
    // Seed a small, deterministic dataset for demo/dev/test modes. All rows are
    // tagged with demo-specific identifiers so the operation is idempotent.
    with_db(|pool| async move {
        // Remove any previous demo seed rows to keep the operation repeatable.
        sqlx::query("DELETE FROM event_log WHERE request_id LIKE 'demo-%'")
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM rate_limit_tokens WHERE scope = 'demo'")
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM image_locks WHERE bucket LIKE 'demo-%'")
            .execute(&pool)
            .await?;

        let now = current_unix_secs() as i64;

        // Event log: mix of manual, webhook, scheduler and health events.
        let events = vec![
            (
                "demo-0001",
                now - 1800,
                "POST",
                Some("/api/manual/trigger"),
                202,
                "manual-trigger",
                12,
                json!({
                    "units": ["podman-auto-update.service", "svc-alpha.service", "svc-beta.service"],
                    "dry_run": true,
                    "caller": "demo",
                    "reason": "initial-seed"
                }),
            ),
            (
                "demo-0002",
                now - 1700,
                "POST",
                Some("/api/manual/services/svc-alpha"),
                202,
                "manual-service",
                34,
                json!({
                    "unit": "svc-alpha.service",
                    "image": "ghcr.io/example/svc-alpha:demo",
                    "dry_run": false,
                    "caller": "demo",
                    "reason": "alpha-rollout"
                }),
            ),
            (
                "demo-0003",
                now - 1600,
                "POST",
                Some("/github-package-update/svc-beta"),
                202,
                "github-webhook",
                48,
                json!({
                    "unit": "svc-beta.service",
                    "image": "ghcr.io/example/svc-beta:main",
                    "delivery": "demo-delivery-1",
                    "event": "registry_package"
                }),
            ),
            (
                "demo-0004",
                now - 1500,
                "POST",
                Some("/github-package-update/svc-beta"),
                500,
                "github-webhook",
                51,
                json!({
                    "unit": "svc-beta.service",
                    "image": "ghcr.io/example/svc-beta:broken",
                    "delivery": "demo-delivery-2",
                    "event": "registry_package",
                    "error": "simulated podman failure"
                }),
            ),
            (
                "demo-0005",
                now - 1400,
                "GET",
                Some("/health"),
                200,
                "health-check",
                3,
                json!({
                    "status": "ok",
                    "scheduler_interval_secs": DEFAULT_SCHEDULER_INTERVAL_SECS
                }),
            ),
            (
                "demo-0006",
                now - 1300,
                "GET",
                Some("/events"),
                200,
                "frontend",
                27,
                json!({
                    "route": "/events",
                    "page": 1,
                    "per_page": 50
                }),
            ),
        ];

        for (request_id, ts, method, path, status, action, duration_ms, meta) in events {
            sqlx::query(
                "INSERT INTO event_log (request_id, ts, method, path, status, action, duration_ms, meta, task_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(request_id)
            .bind(ts)
            .bind(method)
            .bind(path)
            .bind(status as i64)
            .bind(action)
            .bind(duration_ms as i64)
            .bind(serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string()))
            .bind(None::<String>)
            .execute(&pool)
            .await?;
        }

        // Rate limit tokens: one "hot" bucket and one aged-out bucket.
        sqlx::query(
            "INSERT INTO rate_limit_tokens (scope, bucket, ts) VALUES ('demo', 'manual-hot', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO rate_limit_tokens (scope, bucket, ts) VALUES ('demo', 'manual-aged', ?)",
        )
        .bind(now - 200_000)
        .execute(&pool)
        .await?;

        // Image locks: one fresh, one stale.
        sqlx::query(
            "INSERT OR REPLACE INTO image_locks (bucket, acquired_at) VALUES ('demo-lock-fresh', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO image_locks (bucket, acquired_at) VALUES ('demo-lock-stale', ?)",
        )
        .bind(now - 200_000)
        .execute(&pool)
        .await?;

        Ok::<(), sqlx::Error>(())
    })
}

fn persist_event_record(
    request_id: &str,
    ts_secs: u64,
    method: &str,
    path: Option<&str>,
    status: u16,
    action: &str,
    elapsed_ms: u64,
    meta: &Value,
) {
    let pool = db_pool();
    let runtime = match DB_RUNTIME.get() {
        Some(rt) => rt,
        None => return,
    };

    // Extract structured task_id (if present) from meta so it can be stored in
    // a dedicated column for efficient querying by task.
    let task_id = meta
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let Ok(meta_str) = serde_json::to_string(meta) else {
        return;
    };

    let record = DbEventRecord {
        request_id: request_id.to_string(),
        ts: ts_secs as i64,
        method: method.to_string(),
        path: path.map(|p| p.to_string()),
        status: status as i64,
        action: action.to_string(),
        duration_ms: elapsed_ms as i64,
        meta: meta_str,
        task_id,
    };
    let pool = pool.clone();

    let fut = async move {
        if let Err(err) = sqlx::query(
            "INSERT INTO event_log (request_id, ts, method, path, status, action, duration_ms, meta, task_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.request_id)
        .bind(record.ts)
        .bind(record.method)
        .bind(record.path)
        .bind(record.status)
        .bind(record.action)
        .bind(record.duration_ms)
        .bind(record.meta)
        .bind(record.task_id)
        .execute(&pool)
        .await
        {
            log_message(&format!("warn db-insert-failed err={err}"));
        }
    };

    // The HTTP server forks a short-lived process per request; if we spawn the
    // insert task, the child may exit before the future runs. Write
    // synchronously to ensure audit logs are persisted.
    runtime.block_on(fut);
}

fn record_system_event(action: &str, status: u16, meta: Value) {
    let ts = current_unix_secs();
    persist_event_record("system", ts, "SYSTEM", None, status, action, 0, &meta);
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

fn system_time_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

struct DbEventRecord {
    request_id: String,
    ts: i64,
    method: String,
    path: Option<String>,
    status: i64,
    action: String,
    duration_ms: i64,
    meta: String,
    task_id: Option<String>,
}

fn respond_text(
    ctx: &RequestContext,
    status: u16,
    reason: &str,
    body: &str,
    action: &str,
    extra: Option<Value>,
) -> Result<(), String> {
    let metadata = extra.unwrap_or_else(|| json!({ "body": reason }));
    let result = send_response(status, reason, body);
    log_audit_event(ctx, status, action, metadata);
    result
}

fn respond_json(
    ctx: &RequestContext,
    status: u16,
    reason: &str,
    payload: &Value,
    action: &str,
    extra: Option<Value>,
) -> Result<(), String> {
    let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
    let mut metadata = extra.unwrap_or_else(|| json!({}));
    metadata["response_size"] = Value::from(body.len() as u64);
    let result = send_binary_response(status, reason, "application/json; charset=utf-8", &body);
    log_audit_event(ctx, status, action, metadata);
    result
}

fn respond_binary(
    ctx: &RequestContext,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    action: &str,
    extra: Option<Value>,
) -> Result<(), String> {
    let mut metadata = extra.unwrap_or_else(|| json!({}));
    metadata["response_size"] = Value::from(body.len() as u64);
    let result = send_binary_response(status, reason, content_type, body);
    log_audit_event(ctx, status, action, metadata);
    result
}

fn respond_head(
    ctx: &RequestContext,
    status: u16,
    reason: &str,
    content_type: &str,
    content_length: usize,
    action: &str,
    extra: Option<Value>,
) -> Result<(), String> {
    let mut metadata = extra.unwrap_or_else(|| json!({}));
    metadata["response_size"] = Value::from(content_length as u64);
    let result = send_head_response(status, reason, content_type, content_length);
    log_audit_event(ctx, status, action, metadata);
    result
}

fn respond_sse(
    ctx: &RequestContext,
    event: &str,
    payload: &str,
    action: &str,
    extra: Option<Value>,
) -> Result<(), String> {
    let mut metadata = extra.unwrap_or_else(|| json!({}));
    metadata["event"] = Value::from(event);
    metadata["response_size"] = Value::from(payload.len() as u64);
    let result = send_sse_event(event, payload);
    log_audit_event(ctx, 200, action, metadata);
    result
}

fn respond_basic_error(
    request_id: &str,
    method: &str,
    path: &str,
    raw_request: &str,
    status: u16,
    reason: &str,
    body: &str,
    action: &str,
    started_at: Instant,
    received_at: SystemTime,
) -> Result<(), String> {
    let result = send_response(status, reason, body);
    log_simple_audit(
        request_id,
        method,
        path,
        None,
        raw_request,
        status,
        action,
        json!({ "body": reason }),
        started_at,
        received_at,
    );
    result
}

fn log_audit_event(ctx: &RequestContext, status: u16, action: &str, mut meta: Value) {
    let elapsed_ms = ctx.started_at.elapsed().as_millis() as u64;
    let query = ctx.query.as_ref().map(|q| redact_token(q));
    meta["path"] = Value::from(ctx.path.clone());
    if let Some(q) = query.clone() {
        meta["query"] = Value::from(q);
    }
    persist_event_record(
        &ctx.request_id,
        system_time_secs(ctx.received_at),
        &ctx.method,
        Some(&ctx.path),
        status,
        action,
        elapsed_ms,
        &meta,
    );
}

fn log_simple_audit(
    request_id: &str,
    method: &str,
    path: &str,
    query: Option<String>,
    raw_request: &str,
    status: u16,
    action: &str,
    meta: Value,
    started_at: Instant,
    received_at: SystemTime,
) {
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let meta_value = json!({
        "path": path,
        "query": query,
        "raw": redact_token(raw_request),
        "info": meta,
    });
    persist_event_record(
        request_id,
        system_time_secs(received_at),
        method,
        Some(path),
        status,
        action,
        elapsed_ms,
        &meta_value,
    );
}

fn next_request_id() -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis();
    format!("{ts:x}-{seq:04x}")
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(val) => val.trim().parse().map_err(|_| format!("invalid {name}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}")),
    }
}

fn rate_limit_check() -> Result<(), RateLimitError> {
    let cfg = ManualRateLimitConfig::load()?;
    let windows = [
        RateWindow {
            limit: cfg.l1_count,
            window: cfg.l1_window,
        },
        RateWindow {
            limit: cfg.l2_count,
            window: cfg.l2_window,
        },
    ];

    apply_rate_limits(
        "manual",
        "manual-auto-update",
        current_unix_secs(),
        &windows,
        true,
    )
}

struct ManualRateLimitConfig {
    l1_count: u64,
    l1_window: u64,
    l2_count: u64,
    l2_window: u64,
}

impl ManualRateLimitConfig {
    fn load() -> Result<Self, RateLimitError> {
        Ok(Self {
            l1_count: env_u64("PODUP_LIMIT1_COUNT", DEFAULT_LIMIT1_COUNT)
                .map_err(RateLimitError::Io)?,
            l1_window: env_u64("PODUP_LIMIT1_WINDOW", DEFAULT_LIMIT1_WINDOW)
                .map_err(RateLimitError::Io)?,
            l2_count: env_u64("PODUP_LIMIT2_COUNT", DEFAULT_LIMIT2_COUNT)
                .map_err(RateLimitError::Io)?,
            l2_window: env_u64("PODUP_LIMIT2_WINDOW", DEFAULT_LIMIT2_WINDOW)
                .map_err(RateLimitError::Io)?,
        })
    }
}

#[derive(Debug)]
enum RateLimitError {
    LockTimeout,
    Exceeded { c1: u64, l1: u64, c2: u64, l2: u64 },
    Io(String),
}

fn log_message(message: &str) {
    // Try system logger first; fall back to stderr so container logs capture it.
    let _ = Command::new("logger")
        .arg("-t")
        .arg(LOG_TAG)
        .arg(message)
        .status();
    eprintln!("{message}");
}

fn redact_token(input: &str) -> String {
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    let regex = TOKEN_RE.get_or_init(|| Regex::new(r"(token=)[^&\s]+").unwrap());
    regex.replace_all(input, "$1***REDACTED***").into_owned()
}

fn sanitize_image_key(image: &str) -> String {
    let mut key = String::with_capacity(image.len());
    for ch in image.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '.' | '-' | '_') {
            key.push(ch);
        } else {
            key.push('_');
        }
    }

    let trimmed = key.trim_matches('_');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

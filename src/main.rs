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
const DEFAULT_REGISTRY_HOST: &str = "ghcr.io";
const PULL_RETRY_ATTEMPTS: u8 = 3;
const PULL_RETRY_DELAY_SECS: u64 = 5;
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
const ENV_AUDIT_SYNC: &str = "PODUP_AUDIT_SYNC";
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
    let unit = args.get(0).cloned().unwrap_or_default();
    let image = args.get(1).cloned().unwrap_or_default();
    let event = args.get(2).cloned().unwrap_or_default();
    let delivery = args.get(3).cloned().unwrap_or_default();
    let path = args.get(4).cloned().unwrap_or_default();

    if unit.is_empty() || image.is_empty() {
        log_message("500 background-task invalid-args");
        eprintln!("--run-task requires unit and image");
        std::process::exit(1);
    }

    if let Err(err) = run_background_task(&unit, &image, &event, &delivery, &path) {
        log_message(&format!(
            "500 background-task-failed unit={unit} image={image} err={err}"
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
    cmd.stderr(Stdio::null());

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

    let results = trigger_units(&units, opts.dry_run);
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
        opts.dry_run,
        opts.caller.as_deref().unwrap_or("-"),
        opts.reason.as_deref().unwrap_or("-"),
        if ok { "ok" } else { "error" }
    ));
    record_system_event(
        "cli-trigger",
        if ok { 202 } else { 500 },
        json!({
            "dry_run": opts.dry_run,
            "caller": opts.caller,
            "reason": opts.reason,
            "units": units,
            "results": results,
        }),
    );

    if ok {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
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

    match prune_state_dir(Duration::from_secs(retention_secs.max(1)), dry_run) {
        Ok(report) => {
            println!(
                "Removed tokens={} legacy_entries={} stale_locks={} dry_run={}",
                report.tokens_removed, report.legacy_dirs_removed, report.locks_removed, dry_run
            );
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("state prune failed: {err}");
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
    eprintln!("  prune-state [options]        Clean ratelimit databases and locks");
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
    } else if ctx.path == "/api/config" {
        handle_config_api(&ctx)?;
    } else if ctx.path == "/api/settings" {
        handle_settings_api(&ctx)?;
    } else if ctx.path == "/api/events" {
        handle_events_api(&ctx)?;
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
            "SELECT id, request_id, ts, method, path, status, action, duration_ms, meta, created_at FROM event_log{where_sql} ORDER BY ts DESC, id DESC LIMIT ? OFFSET ?"
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
    let result = start_auto_update_unit(&unit)?;
    if result.success() {
        log_message(&format!("202 triggered unit={unit} {}", redacted_line));
        respond_text(
            ctx,
            202,
            "Accepted",
            "auto-update triggered",
            "manual-auto-update",
            Some(json!({ "unit": unit })),
        )?;
    } else {
        let mut message = format!(
            "500 failed unit={unit} {} exit={}",
            redacted_line,
            exit_code_string(&result.status)
        );
        if !result.stderr.is_empty() {
            message.push_str(" stderr=");
            message.push_str(&result.stderr);
        }
        log_message(&message);
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "failed to trigger",
            "manual-auto-update",
            Some(json!({
                "unit": unit,
                "stderr": result.stderr,
            })),
        )?;
    }

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
    let results = trigger_units(&units, dry_run);
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
    };

    let payload = serde_json::to_value(&response).map_err(|e| e.to_string())?;
    respond_json(
        ctx,
        status,
        reason,
        &payload,
        "manual-trigger",
        Some(json!({
            "units": units,
            "dry_run": dry_run,
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
    if !dry_run {
        if let Some(image) = request.image.as_deref() {
            if let Err(err) = pull_container_image(image) {
                respond_text(
                    ctx,
                    500,
                    "InternalServerError",
                    "image pull failed",
                    "manual-service",
                    Some(json!({ "unit": unit, "error": err })),
                )?;
                return Ok(());
            }
        }
    }

    let result = trigger_single_unit(&unit, dry_run);
    let status = if result.status == "triggered" || result.status == "dry-run" {
        202
    } else {
        500
    };
    let reason = if status == 202 {
        "Accepted"
    } else {
        "InternalServerError"
    };

    let response = json!({
        "unit": unit,
        "status": result.status,
        "message": result.message,
        "dry_run": dry_run,
        "caller": request.caller,
        "reason": request.reason,
        "image": request.image,
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
}

#[derive(Default)]
struct ManualCliOptions {
    units: Vec<String>,
    dry_run: bool,
    all: bool,
    caller: Option<String>,
    reason: Option<String>,
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
    if units.is_empty() {
        return Ok(DiscoveryStats::default());
    }

    let mut stats = DiscoveryStats::default();

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

            match unit.source {
                "dir" => stats.dir = stats.dir.saturating_add(1),
                "ps" => stats.ps = stats.ps.saturating_add(1),
                _ => {}
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
            log_message(&format!(
                "info discovery-ok dir={} ps={} total={}",
                stats.dir,
                stats.ps,
                stats.dir.saturating_add(stats.ps)
            ));
            record_system_event(
                "discovery",
                200,
                json!({
                    "status": if stats.dir + stats.ps > 0 { "ok" } else { "empty" },
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

fn manual_unit_list() -> Vec<String> {
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

    for unit in discovered_unit_list() {
        if seen.insert(unit.clone()) {
            units.push(unit);
        }
    }

    units
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
        .all(|r| r.status == "triggered" || r.status == "dry-run")
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

        match start_auto_update_unit(&unit) {
            Ok(result) if result.success() => {
                log_message(&format!(
                    "scheduler triggered unit={unit} iteration={iterations}"
                ));
                record_system_event(
                    "scheduler",
                    202,
                    json!({
                        "unit": unit.clone(),
                        "iteration": iterations,
                        "status": "triggered",
                    }),
                );
            }
            Ok(result) => {
                log_message(&format!(
                    "scheduler failed unit={unit} iteration={iterations} exit={} stderr={}",
                    exit_code_string(&result.status),
                    result.stderr
                ));
                record_system_event(
                    "scheduler",
                    500,
                    json!({
                        "unit": unit.clone(),
                        "iteration": iterations,
                        "status": "failed",
                        "stderr": result.stderr,
                        "exit": exit_code_string(&result.status),
                    }),
                );
            }
            Err(err) => {
                log_message(&format!(
                    "scheduler error unit={unit} iteration={iterations} err={err}"
                ));
                record_system_event(
                    "scheduler",
                    500,
                    json!({
                        "unit": unit.clone(),
                        "iteration": iterations,
                        "status": "error",
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
    let dry_run = request.dry_run;

    match prune_state_dir(Duration::from_secs(retention_secs), dry_run) {
        Ok(report) => {
            let response = json!({
                "tokens_removed": report.tokens_removed,
                "locks_removed": report.locks_removed,
                "legacy_dirs_removed": report.legacy_dirs_removed,
                "dry_run": dry_run,
                "max_age_hours": retention_secs / 3600,
            });
            respond_json(ctx, 200, "OK", &response, "prune-state-api", None)?;
            Ok(())
        }
        Err(err) => {
            respond_text(
                ctx,
                500,
                "InternalServerError",
                "failed to prune state",
                "prune-state-api",
                Some(json!({ "error": err })),
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
        "/" | "/index.html" | "/manual" | "/webhooks" | "/events" | "/maintenance"
        | "/settings" | "/401" => PathBuf::from("index.html"),
        path if path.starts_with("/assets/") => match sanitize_frontend_path(path) {
            Some(p) => p,
            None => return Ok(false),
        },
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

    for unit in manual_unit_list() {
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

    let secret = env::var(ENV_GH_WEBHOOK_SECRET).unwrap_or_default();
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

    let valid_signature = verify_github_signature(signature, &secret, &ctx.body)?;
    if !valid_signature {
        log_message("401 github invalid signature");
        respond_text(
            ctx,
            401,
            "Unauthorized",
            "unauthorized",
            "github-webhook",
            Some(json!({ "reason": "signature" })),
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

    if let Err(err) = spawn_background_task(&unit, &image, &event, &delivery, &ctx.path) {
        log_message(&format!(
            "500 github-dispatch-failed unit={unit} image={image} event={event} delivery={delivery} path={} err={err}",
            ctx.path
        ));
        respond_text(
            ctx,
            500,
            "InternalServerError",
            "failed to dispatch",
            "github-webhook",
            Some(json!({ "unit": unit, "image": image })),
        )?;
        return Ok(());
    }

    respond_text(
        ctx,
        202,
        "Accepted",
        "auto-update queued",
        "github-webhook",
        Some(json!({ "unit": unit, "image": image, "delivery": delivery })),
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
    stderr: String,
}

impl CommandExecResult {
    fn success(&self) -> bool {
        self.status.success()
    }
}

fn run_quiet_command(mut command: Command) -> Result<CommandExecResult, String> {
    let output = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    Ok(CommandExecResult {
        status: output.status,
        stderr,
    })
}

fn podman_health() -> Result<(), String> {
    PODMAN_HEALTH
        .get_or_init(|| {
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
    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user").arg("start").arg(unit);
        cmd
    })
}

fn restart_unit(unit: &str) -> Result<CommandExecResult, String> {
    run_quiet_command({
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user").arg("restart").arg(unit);
        cmd
    })
}

fn pull_container_image(image: &str) -> Result<(), String> {
    for attempt in 1..=PULL_RETRY_ATTEMPTS {
        let result = run_quiet_command({
            let mut cmd = Command::new("podman");
            cmd.arg("pull").arg(image);
            cmd
        })?;
        if result.success() {
            return Ok(());
        }

        if attempt == PULL_RETRY_ATTEMPTS {
            let mut message = exit_code_string(&result.status);
            if !result.stderr.is_empty() {
                message.push_str(": ");
                message.push_str(&result.stderr);
            }
            return Err(message);
        }

        thread::sleep(Duration::from_secs(PULL_RETRY_DELAY_SECS));
    }

    Err("unreachable".into())
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
) -> Result<(), String> {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_str().ok_or_else(|| "invalid exe path".to_string())?;
    let suffix = sanitize_image_key(delivery);
    let unit_name = format!("webhook-task-{}", suffix);

    log_message(&format!(
        "debug github-dispatch-launch unit={unit} image={image} event={event} delivery={delivery} path={path} exe={exe_str} task-unit={unit_name}"
    ));

    let args = build_systemd_run_args(&unit_name, exe_str, unit, image, event, delivery, path);

    if let Ok(snapshot) = env::var(ENV_SYSTEMD_RUN_SNAPSHOT) {
        fs::write(snapshot, args.join("\n")).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let status = Command::new("systemd-run")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err(exit_code_string(&status))
    }
}

fn build_systemd_run_args(
    unit_name: &str,
    exe: &str,
    unit: &str,
    image: &str,
    event: &str,
    delivery: &str,
    path: &str,
) -> Vec<String> {
    vec![
        "--user".into(),
        "--collect".into(),
        "--quiet".into(),
        format!("--unit={unit_name}"),
        exe.to_string(),
        "--run-task".into(),
        unit.to_string(),
        image.to_string(),
        event.to_string(),
        delivery.to_string(),
        path.to_string(),
    ]
}

fn run_background_task(
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
            return Ok(());
        }
        Err(RateLimitError::Exceeded { c1, l1, .. }) => {
            log_message(&format!(
                "429 github-rate-limit image={image} count={c1}/{l1} event={event} delivery={delivery} path={path}"
            ));
            return Ok(());
        }
        Err(RateLimitError::Io(err)) => return Err(err),
    };

    let _guard = guard;

    if let Err(err) = pull_container_image(image) {
        log_message(&format!(
            "500 github-image-pull-failed unit={unit} image={image} event={event} delivery={delivery} path={path} err={err}"
        ));
        return Ok(());
    }

    match restart_unit(unit) {
        Ok(result) if result.success() => {
            log_message(&format!(
                "202 github-triggered unit={unit} image={image} event={event} delivery={delivery} path={path}"
            ));
            prune_images_silently();
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
        }
        Err(err) => {
            log_message(&format!(
                "500 github-restart-error unit={unit} image={image} event={event} delivery={delivery} path={path} err={err}"
            ));
        }
    }

    Ok(())
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
    fn systemd_run_args_match_expected() {
        let args = build_systemd_run_args(
            "webhook-task-demo",
            "/usr/bin/webhook",
            "demo.service",
            "ghcr.io/example/demo:main",
            "registry_package",
            "delivery123",
            "/github-package-update/demo",
        );

        assert_eq!(args[0], "--user");
        assert_eq!(args[1], "--collect");
        assert_eq!(args[2], "--quiet");
        assert_eq!(args[3], "--unit=webhook-task-demo");
        assert_eq!(args[4], "/usr/bin/webhook");
        assert_eq!(args[5], "--run-task");
        assert_eq!(args[6], "demo.service");
        assert_eq!(args[7], "ghcr.io/example/demo:main");
        assert_eq!(args[8], "registry_package");
        assert_eq!(args[9], "delivery123");
        assert_eq!(args[10], "/github-package-update/demo");
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

fn verify_github_signature(signature: &str, secret: &str, body: &[u8]) -> Result<bool, String> {
    let Some(hex_part) = signature.strip_prefix("sha256=") else {
        return Ok(false);
    };

    let provided = decode(hex_part).map_err(|_| "invalid signature hex".to_string())?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(body);

    let verifier = mac.clone();
    if verifier.verify_slice(&provided).is_ok() {
        return Ok(true);
    }

    use hex::ToHex;
    use sha2::Digest;
    let expected = mac.finalize().into_bytes();
    let body_hash = sha2::Sha256::digest(body);

    let debug_path = env::var(ENV_DEBUG_PAYLOAD_PATH)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| {
            let default = Path::new(DEFAULT_STATE_DIR).join("last_payload.bin");
            default.to_string_lossy().into_owned()
        });

    if let Ok(mut file) = File::create(&debug_path) {
        if let Err(err) = file.write_all(body) {
            log_message(&format!(
                "debug payload-write-failed path={} err={}",
                debug_path, err
            ));
        }
    }

    log_message(&format!(
        "signature-mismatch provided={} expected={} body-len={} body-sha256={} payload-dump={}",
        hex_part,
        expected.encode_hex::<String>(),
        body.len(),
        body_hash.encode_hex::<String>(),
        debug_path
    ));

    Ok(false)
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
                "INSERT INTO event_log (request_id, ts, method, path, status, action, duration_ms, meta) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(request_id)
            .bind(ts)
            .bind(method)
            .bind(path)
            .bind(status as i64)
            .bind(action)
            .bind(duration_ms as i64)
            .bind(serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string()))
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
    };
    let pool = pool.clone();

    let fut = async move {
        if let Err(err) = sqlx::query(
            "INSERT INTO event_log (request_id, ts, method, path, status, action, duration_ms, meta) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.request_id)
        .bind(record.ts)
        .bind(record.method)
        .bind(record.path)
        .bind(record.status)
        .bind(record.action)
        .bind(record.duration_ms)
        .bind(record.meta)
        .execute(&pool)
        .await
        {
            log_message(&format!("warn db-insert-failed err={err}"));
        }
    };

    // Discovery and audit-critical actions run synchronously to avoid being dropped;
    // other actions can be spawned.
    if audit_sync_mode() || action == "discovery" {
        runtime.block_on(fut);
    } else {
        runtime.spawn(fut);
    }
}

fn audit_sync_mode() -> bool {
    static SYNC_MODE: OnceLock<bool> = OnceLock::new();
    *SYNC_MODE.get_or_init(|| {
        env::var(ENV_AUDIT_SYNC)
            .ok()
            .map(|value| {
                let normalized = value.trim().to_ascii_lowercase();
                matches!(normalized.as_str(), "1" | "true" | "yes")
            })
            .unwrap_or(false)
    })
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

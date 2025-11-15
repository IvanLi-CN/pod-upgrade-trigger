use hex::decode;
use hmac::{Hmac, Mac};
use libc::{self, LOCK_EX, LOCK_NB, LOCK_UN};
use regex::Regex;
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

const LOG_TAG: &str = "webhook-auto-update";
const DEFAULT_STATE_DIR: &str = "/srv/pod-upgrade-trigger";
const DEFAULT_WEB_DIST_DIR: &str = "web/dist";
const DEFAULT_CONTAINER_DIR: &str = "/home/<user>/.config/containers/systemd";
const GITHUB_ROUTE_PREFIX: &str = "github-package-update";
const DEFAULT_LIMIT1_COUNT: u64 = 2;
const DEFAULT_LIMIT1_WINDOW: u64 = 600; // 10 minutes
const DEFAULT_LIMIT2_COUNT: u64 = 10;
const DEFAULT_LIMIT2_WINDOW: u64 = 18_000; // 5 hours
const GITHUB_IMAGE_LIMIT_COUNT: u64 = 60;
const GITHUB_IMAGE_LIMIT_WINDOW: u64 = 3_600; // 1 hour
const GITHUB_IMAGE_LIMIT_SUBDIR: &str = "github-image-limits";
const GITHUB_IMAGE_LOCK_SUBDIR: &str = "github-image-locks";
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MANUAL_UNIT: &str = "podman-auto-update.service";
const DEFAULT_REGISTRY_HOST: &str = "ghcr.io";
const PULL_RETRY_ATTEMPTS: u8 = 3;
const PULL_RETRY_DELAY_SECS: u64 = 5;

type HmacSha256 = Hmac<Sha256>;

struct RequestContext {
    method: String,
    path: String,
    query: Option<String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    raw_request: String,
}

fn manual_auto_update_unit() -> String {
    env::var("MANUAL_AUTO_UPDATE_UNIT").unwrap_or_else(|_| DEFAULT_MANUAL_UNIT.to_string())
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
    let _exe = args.next();

    if let Some(mode) = args.next() {
        if mode == "--run-task" {
            let unit = args.next().unwrap_or_default();
            let image = args.next().unwrap_or_default();
            let event = args.next().unwrap_or_default();
            let delivery = args.next().unwrap_or_default();
            let path = args.next().unwrap_or_default();

            if unit.is_empty() || image.is_empty() {
                log_message("500 background-task invalid-args");
                std::process::exit(1);
            }

            if let Err(err) = run_background_task(&unit, &image, &event, &delivery, &path) {
                log_message(&format!(
                    "500 background-task-failed unit={unit} image={image} err={err}"
                ));
                std::process::exit(1);
            }
            return;
        }
    }

    if let Err(err) = handle_connection() {
        log_message(&format!("500 internal-error {err}"));
        let _ = write_response(500, "InternalServerError", "internal error");
    }
}

fn handle_connection() -> Result<(), String> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| e.to_string())?;
    let request_line = request_line.trim_end_matches(['\r', '\n']).to_string();

    let (method, raw_target) = parse_request_line(&request_line);
    if method.is_empty() || raw_target.is_empty() {
        log_message(&format!("400 bad-request {}", redact_token(&request_line)));
        send_response(400, "BadRequest", "bad request")?;
        return Ok(());
    }

    let (path, query) = match parse_target(&raw_target) {
        Ok(parts) => parts,
        Err(e) => {
            log_message(&format!("400 bad-request {}", redact_token(&request_line)));
            send_response(400, "BadRequest", &e)?;
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
    } else {
        reader
            .read_to_end(&mut body)
            .map_err(|e| format!("failed to read body: {e}"))?;
    }

    let ctx = RequestContext {
        method,
        path,
        query,
        headers,
        body,
        raw_request: request_line,
    };

    if ctx.method == "GET" && ctx.path == "/health" {
        log_message("health ok");
        send_response(200, "OK", "ok")?;
    } else if ctx.method == "GET" && ctx.path == "/sse/hello" {
        handle_hello_sse(&ctx)?;
    } else if is_github_route(&ctx.path) {
        handle_github_request(&ctx)?;
    } else if ctx.path == "/auto-update" {
        handle_manual_request(&ctx)?;
    } else if try_serve_frontend(&ctx)? {
        // served static asset
    } else {
        log_message(&format!("404 {}", redact_token(&ctx.raw_request)));
        send_response(404, "NotFound", "not found")?;
    }

    Ok(())
}

fn handle_hello_sse(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "GET" {
        send_response(405, "MethodNotAllowed", "method not allowed")?;
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
    send_sse_event("hello", &payload.to_string())
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
        send_response(405, "MethodNotAllowed", "method not allowed")?;
        return Ok(());
    }

    let redacted_line = redact_token(&ctx.raw_request);

    let token = ctx
        .query
        .as_deref()
        .and_then(extract_query_token)
        .unwrap_or_default();

    let expected = env::var("WEBHOOK_TOKEN").unwrap_or_default();
    if expected.is_empty() || token.is_empty() || token != expected {
        log_message(&format!("401 {}", redacted_line));
        send_response(401, "Unauthorized", "unauthorized")?;
        return Ok(());
    }

    if !enforce_rate_limit(&redacted_line)? {
        return Ok(());
    }

    let unit = manual_auto_update_unit();
    let result = start_auto_update_unit(&unit)?;
    if result.success() {
        log_message(&format!("202 triggered unit={unit} {}", redacted_line));
        send_response(202, "Accepted", "auto-update triggered")?;
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
        send_response(500, "InternalServerError", "failed to trigger")?;
    }

    Ok(())
}

fn try_serve_frontend(ctx: &RequestContext) -> Result<bool, String> {
    if ctx.method != "GET" && ctx.method != "HEAD" {
        return Ok(false);
    }
    let head_only = ctx.method == "HEAD";

    let relative = match ctx.path.as_str() {
        "/" | "/index.html" => PathBuf::from("index.html"),
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
            send_head_response(200, "OK", content_type, len as usize)?;
            return Ok(true);
        }

        let body = fs::read(&asset_path)
            .map_err(|e| format!("failed to read asset {}: {e}", asset_path.display()))?;
        send_binary_response(200, "OK", content_type, &body)?;
        return Ok(true);
    }

    if relative == PathBuf::from("index.html") {
        log_message("500 web-ui missing index.html");
        send_response(500, "InternalServerError", "web ui not built")?;
        return Ok(true);
    }

    log_message(&format!(
        "404 asset-not-found path={} relative={}",
        ctx.path,
        relative.display()
    ));
    send_response(404, "NotFound", "asset not found")?;
    Ok(true)
}

fn frontend_dist_dir() -> PathBuf {
    env::var("WEBHOOK_WEB_DIST")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(DEFAULT_STATE_DIR).join(DEFAULT_WEB_DIST_DIR))
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

fn handle_github_request(ctx: &RequestContext) -> Result<(), String> {
    if ctx.method != "POST" {
        log_message(&format!(
            "405 github-method-not-allowed {}",
            ctx.raw_request
        ));
        send_response(405, "MethodNotAllowed", "method not allowed")?;
        return Ok(());
    }

    let secret = env::var("GITHUB_WEBHOOK_SECRET").unwrap_or_default();
    if secret.is_empty() {
        log_message("500 github-misconfigured missing secret");
        send_response(500, "InternalServerError", "server misconfigured")?;
        return Ok(());
    }

    let signature = match ctx.headers.get("x-hub-signature-256") {
        Some(value) => value,
        None => {
            log_message("401 github missing signature");
            send_response(401, "Unauthorized", "unauthorized")?;
            return Ok(());
        }
    };

    let valid_signature = verify_github_signature(signature, &secret, &ctx.body)?;
    if !valid_signature {
        log_message("401 github invalid signature");
        send_response(401, "Unauthorized", "unauthorized")?;
        return Ok(());
    }

    let event = ctx
        .headers
        .get("x-github-event")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());

    if !github_event_allowed(&event) {
        log_message(&format!("202 github event-ignored event={event}"));
        send_response(202, "Accepted", "event ignored")?;
        return Ok(());
    }

    let Some(unit) = lookup_unit_from_path(&ctx.path) else {
        log_message(&format!(
            "202 github event={event} path={} no-unit-mapped",
            ctx.path
        ));
        send_response(202, "Accepted", "event ignored")?;
        return Ok(());
    };

    let image = match extract_container_image(&ctx.body) {
        Ok(img) => img,
        Err(reason) => {
            log_message(&format!("202 github event={event} skipped reason={reason}"));
            send_response(202, "Accepted", "event ignored")?;
            return Ok(());
        }
    };

    if let Some(expected) = unit_configured_image(&unit) {
        if !images_match(&image, &expected) {
            log_message(&format!(
                "202 github event={event} unit={unit} image={image} expected={expected} skipped=tag-mismatch"
            ));
            send_response(202, "Accepted", "tag mismatch")?;
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
                send_response(429, "Too Many Requests", "rate limited")?;
                return Ok(());
            }
            RateLimitError::Exceeded { c1, l1, .. } => {
                log_message(&format!(
                    "429 github-rate-limit image={image} count={c1}/{l1} event={event}"
                ));
                send_response(429, "Too Many Requests", "rate limited")?;
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
        send_response(500, "InternalServerError", "failed to dispatch")?;
        return Ok(());
    }

    send_response(202, "Accepted", "auto-update queued")
}

fn enforce_rate_limit(context: &str) -> Result<bool, String> {
    match rate_limit_check() {
        Ok(()) => Ok(true),
        Err(RateLimitError::LockTimeout) => {
            log_message("429 rate-limit lock-timeout");
            send_response(429, "Too Many Requests", "rate limited")?;
            Ok(false)
        }
        Err(RateLimitError::Exceeded { c1, l1, c2, l2 }) => {
            log_message(&format!(
                "429 rate-limit c1={c1}/{l1} c2={c2}/{l2} ({context})"
            ));
            send_response(429, "Too Many Requests", "rate limited")?;
            Ok(false)
        }
        Err(RateLimitError::Io(err)) => Err(err),
    }
}

struct ImageTaskGuard {
    _lock: FlockGuard,
}

fn check_github_image_limit(image: &str) -> Result<(), RateLimitError> {
    let state_dir = env::var("WEBHOOK_STATE_DIR").unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
    let state_path = Path::new(&state_dir);
    fs::create_dir_all(state_path).map_err(|e| RateLimitError::Io(e.to_string()))?;

    let key = sanitize_image_key(image);

    let limit_dir = state_path.join(GITHUB_IMAGE_LIMIT_SUBDIR);
    fs::create_dir_all(&limit_dir).map_err(|e| RateLimitError::Io(e.to_string()))?;
    let db_path = limit_dir.join(format!("{key}.db"));

    let lock_dir = state_path.join(GITHUB_IMAGE_LOCK_SUBDIR);
    fs::create_dir_all(&lock_dir).map_err(|e| RateLimitError::Io(e.to_string()))?;
    let lock_path = lock_dir.join(format!("{key}.lock"));

    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| RateLimitError::Io(e.to_string()))?;

    let _guard = FlockGuard::lock_blocking(lock_file)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    let cutoff = now.saturating_sub(GITHUB_IMAGE_LIMIT_WINDOW);

    let mut entries: Vec<u64> = fs::read_to_string(&db_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .collect();

    entries.retain(|&ts| ts >= cutoff);

    let count = entries.len() as u64;
    if count >= GITHUB_IMAGE_LIMIT_COUNT {
        return Err(RateLimitError::Exceeded {
            c1: count,
            l1: GITHUB_IMAGE_LIMIT_COUNT,
            c2: count,
            l2: GITHUB_IMAGE_LIMIT_COUNT,
        });
    }

    let mut file = File::create(&db_path).map_err(|e| RateLimitError::Io(e.to_string()))?;
    for ts in entries {
        writeln!(file, "{ts}").map_err(|e| RateLimitError::Io(e.to_string()))?;
    }

    Ok(())
}

fn enforce_github_image_limit(image: &str) -> Result<ImageTaskGuard, RateLimitError> {
    let state_dir = env::var("WEBHOOK_STATE_DIR").unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
    let state_path = Path::new(&state_dir);
    fs::create_dir_all(state_path).map_err(|e| RateLimitError::Io(e.to_string()))?;

    let key = sanitize_image_key(image);

    let limit_dir = state_path.join(GITHUB_IMAGE_LIMIT_SUBDIR);
    fs::create_dir_all(&limit_dir).map_err(|e| RateLimitError::Io(e.to_string()))?;
    let db_path = limit_dir.join(format!("{key}.db"));

    let lock_dir = state_path.join(GITHUB_IMAGE_LOCK_SUBDIR);
    fs::create_dir_all(&lock_dir).map_err(|e| RateLimitError::Io(e.to_string()))?;
    let lock_path = lock_dir.join(format!("{key}.lock"));

    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| RateLimitError::Io(e.to_string()))?;

    let guard = FlockGuard::lock_blocking(lock_file)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    let cutoff = now.saturating_sub(GITHUB_IMAGE_LIMIT_WINDOW);

    let mut entries: Vec<u64> = fs::read_to_string(&db_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .collect();

    entries.retain(|&ts| ts >= cutoff);

    let count = entries.len() as u64;
    if count >= GITHUB_IMAGE_LIMIT_COUNT {
        drop(guard);
        return Err(RateLimitError::Exceeded {
            c1: count,
            l1: GITHUB_IMAGE_LIMIT_COUNT,
            c2: count,
            l2: GITHUB_IMAGE_LIMIT_COUNT,
        });
    }

    entries.push(now);

    let mut file = File::create(&db_path).map_err(|e| RateLimitError::Io(e.to_string()))?;
    for ts in entries {
        writeln!(file, "{ts}").map_err(|e| RateLimitError::Io(e.to_string()))?;
    }

    Ok(ImageTaskGuard { _lock: guard })
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

    let status = Command::new("systemd-run")
        .arg("--user")
        .arg("--collect")
        .arg("--quiet")
        .arg(format!("--unit={unit_name}"))
        .arg(exe_str)
        .arg("--run-task")
        .arg(unit)
        .arg(image)
        .arg(event)
        .arg(delivery)
        .arg(path)
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
    use std::io::Write;
    use tempfile::NamedTempFile;

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

    let debug_path = env::var("WEBHOOK_DEBUG_PAYLOAD_PATH")
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

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(val) => val.trim().parse().map_err(|_| format!("invalid {name}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}")),
    }
}

fn rate_limit_check() -> Result<(), RateLimitError> {
    let state_dir = env::var("WEBHOOK_STATE_DIR").unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
    let state_path = Path::new(&state_dir);
    fs::create_dir_all(state_path).map_err(|e| RateLimitError::Io(e.to_string()))?;

    let db_path = state_path.join("ratelimit.db");
    let lock_path = state_path.join("ratelimit.lock");

    let l1_count =
        env_u64("WEBHOOK_LIMIT1_COUNT", DEFAULT_LIMIT1_COUNT).map_err(RateLimitError::Io)?;
    let l1_window =
        env_u64("WEBHOOK_LIMIT1_WINDOW", DEFAULT_LIMIT1_WINDOW).map_err(RateLimitError::Io)?;
    let l2_count =
        env_u64("WEBHOOK_LIMIT2_COUNT", DEFAULT_LIMIT2_COUNT).map_err(RateLimitError::Io)?;
    let l2_window =
        env_u64("WEBHOOK_LIMIT2_WINDOW", DEFAULT_LIMIT2_WINDOW).map_err(RateLimitError::Io)?;

    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| RateLimitError::Io(e.to_string()))?;

    let _guard = FlockGuard::lock(lock_file, LOCK_TIMEOUT)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();

    let cutoff_l1 = now.saturating_sub(l1_window);
    let cutoff_l2 = now.saturating_sub(l2_window);

    let mut entries: Vec<u64> = fs::read_to_string(&db_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .collect();

    entries.retain(|&ts| ts >= cutoff_l2);

    let c1 = entries.iter().filter(|&&ts| ts >= cutoff_l1).count() as u64;
    let c2 = entries.len() as u64;

    if c1 >= l1_count || c2 >= l2_count {
        return Err(RateLimitError::Exceeded {
            c1,
            l1: l1_count,
            c2,
            l2: l2_count,
        });
    }

    entries.push(now);

    let mut file = File::create(&db_path).map_err(|e| RateLimitError::Io(e.to_string()))?;
    for ts in entries {
        writeln!(file, "{ts}").map_err(|e| RateLimitError::Io(e.to_string()))?;
    }

    Ok(())
}

struct FlockGuard {
    file: File,
}

impl FlockGuard {
    fn lock(file: File, timeout: Duration) -> Result<Self, RateLimitError> {
        let fd = file.as_raw_fd();
        let start = Instant::now();
        loop {
            let result = unsafe { libc::flock(fd, LOCK_EX | LOCK_NB) };
            if result == 0 {
                break;
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                if start.elapsed() >= timeout {
                    return Err(RateLimitError::LockTimeout);
                }
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            return Err(RateLimitError::Io(err.to_string()));
        }
        Ok(Self { file })
    }

    fn lock_blocking(file: File) -> Result<Self, RateLimitError> {
        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, LOCK_EX) };
        if result == 0 {
            Ok(Self { file })
        } else {
            Err(RateLimitError::Io(io::Error::last_os_error().to_string()))
        }
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), LOCK_UN) };
    }
}

enum RateLimitError {
    LockTimeout,
    Exceeded { c1: u64, l1: u64, c2: u64, l2: u64 },
    Io(String),
}

fn log_message(message: &str) {
    let _ = Command::new("logger")
        .arg("-t")
        .arg(LOG_TAG)
        .arg(message)
        .status();
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

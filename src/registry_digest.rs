use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const AUTH_JSON_REL_PATH: &str = ".config/containers/auth.json";
const DOCKER_CONTENT_DIGEST_HEADER: &str = "docker-content-digest";

pub(crate) const ENV_REGISTRY_DIGEST_CACHE_TTL_SECS: &str = "PODUP_REGISTRY_DIGEST_CACHE_TTL_SECS";
pub(crate) const DEFAULT_REGISTRY_DIGEST_CACHE_TTL_SECS: u64 = 600;
const ENV_REGISTRY_DIGEST_MOCK: &str = "PODUP_REGISTRY_DIGEST_MOCK";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegistryDigestStatus {
    Ok,
    Error,
}

impl RegistryDigestStatus {
    fn as_str(self) -> &'static str {
        match self {
            RegistryDigestStatus::Ok => "ok",
            RegistryDigestStatus::Error => "error",
        }
    }

    fn from_db(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "ok" => RegistryDigestStatus::Ok,
            _ => RegistryDigestStatus::Error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegistryDigestRecord {
    pub image: String,
    pub digest: Option<String>,
    pub checked_at: i64,
    pub status: RegistryDigestStatus,
    pub error: Option<String>,
    pub stale: bool,
    pub from_cache: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegistryDigestError {
    InvalidImage,
    Timeout,
    Unauthorized,
    AuthMissing,
    AuthParse,
    ChallengeParse,
    BadResponse,
    DigestMissing,
    Io,
    Json,
}

impl RegistryDigestError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            RegistryDigestError::InvalidImage => "invalid-image",
            RegistryDigestError::Timeout => "timeout",
            RegistryDigestError::Unauthorized => "unauthorized",
            RegistryDigestError::AuthMissing => "auth-missing",
            RegistryDigestError::AuthParse => "auth-parse",
            RegistryDigestError::ChallengeParse => "challenge-parse",
            RegistryDigestError::BadResponse => "bad-response",
            RegistryDigestError::DigestMissing => "digest-missing",
            RegistryDigestError::Io => "io-error",
            RegistryDigestError::Json => "json-error",
        }
    }
}

#[derive(Clone, Debug)]
struct BasicCredentials {
    username: String,
    password: String,
}

#[derive(Clone, Debug)]
struct ParsedImageRef {
    scheme: String,
    registry: String, // host[:port], no scheme, lowercased host.
    repo: String,     // path without tag
    tag: String,
    normalized_image: String, // registry/repo:tag (no scheme)
}

pub(crate) fn registry_digest_cache_ttl_secs() -> u64 {
    env::var(ENV_REGISTRY_DIGEST_CACHE_TTL_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_REGISTRY_DIGEST_CACHE_TTL_SECS)
}

pub(crate) async fn get_cached_remote_digest(
    pool: &SqlitePool,
    image: &str,
    ttl_secs: u64,
) -> Result<Option<RegistryDigestRecord>, RegistryDigestError> {
    let parsed = parse_image_ref(image)?;
    let row = sqlx::query(
        "SELECT image, digest, checked_at, status, error FROM registry_digest_cache WHERE image = ?",
    )
    .bind(&parsed.normalized_image)
    .fetch_optional(pool)
    .await
    .map_err(|_| RegistryDigestError::BadResponse)?;

    let Some(row) = row else { return Ok(None) };

    let image: String = row.get("image");
    let digest: Option<String> = row.get("digest");
    let checked_at: i64 = row.get("checked_at");
    let status_raw: String = row.get("status");
    let status = RegistryDigestStatus::from_db(&status_raw);
    let error: Option<String> = row.get("error");

    let stale = compute_stale(checked_at, ttl_secs, status);
    Ok(Some(RegistryDigestRecord {
        image,
        digest,
        checked_at,
        status,
        error,
        stale,
        from_cache: true,
    }))
}

pub(crate) async fn resolve_remote_manifest_digest(
    pool: &SqlitePool,
    image: &str,
    ttl_secs: u64,
    force_refresh: bool,
) -> RegistryDigestRecord {
    let parsed = match parse_image_ref(image) {
        Ok(value) => value,
        Err(err) => {
            return RegistryDigestRecord {
                image: image.trim().to_string(),
                digest: None,
                checked_at: crate::current_unix_secs() as i64,
                status: RegistryDigestStatus::Error,
                error: Some(err.code().to_string()),
                stale: true,
                from_cache: false,
            };
        }
    };

    let cached = match get_cached_row(pool, &parsed.normalized_image).await {
        Ok(row) => row,
        Err(_) => None,
    };

    if let Some(row) = cached.as_ref() {
        let expired = is_expired(row.checked_at, ttl_secs);
        let stale = expired || row.status != RegistryDigestStatus::Ok;
        if !force_refresh {
            return RegistryDigestRecord {
                image: row.image.clone(),
                digest: row.digest.clone(),
                checked_at: row.checked_at,
                status: row.status,
                error: row.error.clone(),
                stale,
                from_cache: true,
            };
        }
    }

    let previous_digest = cached.as_ref().and_then(|r| r.digest.clone());
    match refresh_remote_manifest_digest(&parsed).await {
        Ok(digest) => {
            let record = upsert_cache_row(
                pool,
                &parsed.normalized_image,
                Some(&digest),
                RegistryDigestStatus::Ok,
                None,
            )
            .await;
            match record {
                Ok(record) => RegistryDigestRecord {
                    from_cache: false,
                    ..record
                },
                Err(_) => RegistryDigestRecord {
                    image: parsed.normalized_image.clone(),
                    digest: Some(digest),
                    checked_at: crate::current_unix_secs() as i64,
                    status: RegistryDigestStatus::Ok,
                    error: None,
                    stale: false,
                    from_cache: false,
                },
            }
        }
        Err(err) => {
            let err_code = err.code();
            let _ = upsert_cache_row(
                pool,
                &parsed.normalized_image,
                previous_digest.as_deref(),
                RegistryDigestStatus::Error,
                Some(err_code),
            )
            .await;

            RegistryDigestRecord {
                image: parsed.normalized_image.clone(),
                digest: previous_digest,
                checked_at: crate::current_unix_secs() as i64,
                status: RegistryDigestStatus::Error,
                error: Some(err_code.to_string()),
                stale: true,
                from_cache: false,
            }
        }
    }
}

async fn refresh_remote_manifest_digest(
    image: &ParsedImageRef,
) -> Result<String, RegistryDigestError> {
    if env::var("PODUP_ENV")
        .ok()
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
        .is_some_and(|v| v == "test" || v == "testing")
    {
        if let Ok(raw) = env::var(ENV_REGISTRY_DIGEST_MOCK) {
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                if let Some(obj) = value.as_object() {
                    if let Some(entry) = obj.get(&image.normalized_image) {
                        if let Some(digest) = entry.as_str() {
                            let trimmed = digest.trim();
                            if trimmed.starts_with("sha256:") {
                                return Ok(trimmed.to_string());
                            }
                            return Err(RegistryDigestError::DigestMissing);
                        }
                        if entry.is_null() {
                            return Err(RegistryDigestError::DigestMissing);
                        }
                        if let Some(err_obj) = entry.as_object() {
                            if let Some(code) = err_obj.get("error").and_then(|v| v.as_str()) {
                                return Err(match code.trim() {
                                    "timeout" => RegistryDigestError::Timeout,
                                    "unauthorized" => RegistryDigestError::Unauthorized,
                                    "auth-missing" => RegistryDigestError::AuthMissing,
                                    "auth-parse" => RegistryDigestError::AuthParse,
                                    "challenge-parse" => RegistryDigestError::ChallengeParse,
                                    "bad-response" => RegistryDigestError::BadResponse,
                                    "digest-missing" => RegistryDigestError::DigestMissing,
                                    "io-error" => RegistryDigestError::Io,
                                    "json-error" => RegistryDigestError::Json,
                                    _ => RegistryDigestError::BadResponse,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let client = registry_http_client().map_err(|_| RegistryDigestError::BadResponse)?;
    let manifest_url = format!(
        "{}://{}/v2/{}/manifests/{}",
        image.scheme, image.registry, image.repo, image.tag
    );

    let response = client
        .head(&manifest_url)
        .headers(manifest_accept_headers())
        .send()
        .await
        .map_err(map_reqwest_error)?;

    if response.status().is_success() {
        return read_digest_header(&response.headers());
    }

    if response.status() != StatusCode::UNAUTHORIZED {
        return Err(map_status_to_error(response.status()));
    }

    let challenge_headers = response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>();

    if let Some(challenge) = challenge_headers
        .iter()
        .find(|h| h.trim_start().to_ascii_lowercase().starts_with("bearer "))
    {
        let bearer = parse_www_authenticate_bearer(challenge)?;
        let creds = load_basic_credentials_for_registry(&image.registry)?;
        let token = fetch_bearer_token(&client, &bearer, &creds).await?;

        let retry = client
            .head(&manifest_url)
            .headers(manifest_accept_headers())
            .bearer_auth(token)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        if retry.status().is_success() {
            return read_digest_header(&retry.headers());
        }
        return Err(map_status_to_error(retry.status()));
    }

    if challenge_headers
        .iter()
        .any(|h| h.trim_start().to_ascii_lowercase().starts_with("basic "))
    {
        let creds = load_basic_credentials_for_registry(&image.registry)?;
        let retry = client
            .head(&manifest_url)
            .headers(manifest_accept_headers())
            .basic_auth(creds.username, Some(creds.password))
            .send()
            .await
            .map_err(map_reqwest_error)?;

        if retry.status().is_success() {
            return read_digest_header(&retry.headers());
        }
        return Err(map_status_to_error(retry.status()));
    }

    Err(RegistryDigestError::Unauthorized)
}

fn map_reqwest_error(err: reqwest::Error) -> RegistryDigestError {
    if err.is_timeout() {
        return RegistryDigestError::Timeout;
    }
    RegistryDigestError::BadResponse
}

fn map_status_to_error(status: StatusCode) -> RegistryDigestError {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return RegistryDigestError::Unauthorized;
    }
    RegistryDigestError::BadResponse
}

fn registry_http_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(Duration::from_secs(3))
        .pool_max_idle_per_host(0)
        .build()
}

fn manifest_accept_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    let accept = "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json";
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_str(accept).unwrap_or_else(|_| HeaderValue::from_static("*/*")),
    );
    headers
}

fn read_digest_header(headers: &HeaderMap) -> Result<String, RegistryDigestError> {
    let value = headers
        .get(DOCKER_CONTENT_DIGEST_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or(RegistryDigestError::DigestMissing)?;
    Ok(value.to_string())
}

#[derive(Clone, Debug)]
struct BearerChallenge {
    realm: String,
    service: Option<String>,
    scope: Option<String>,
}

fn parse_www_authenticate_bearer(header: &str) -> Result<BearerChallenge, RegistryDigestError> {
    let trimmed = header.trim();
    let rest = trimmed
        .splitn(2, ' ')
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string();

    let params = parse_auth_params(&rest);
    let realm = params
        .get("realm")
        .cloned()
        .filter(|v| !v.is_empty())
        .ok_or(RegistryDigestError::ChallengeParse)?;

    Ok(BearerChallenge {
        realm,
        service: params.get("service").cloned().filter(|v| !v.is_empty()),
        scope: params.get("scope").cloned().filter(|v| !v.is_empty()),
    })
}

fn parse_auth_params(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in input.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let mut value = v.trim().to_string();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            value = value[1..value.len() - 1].to_string();
        }
        out.insert(key, value);
    }
    out
}

async fn fetch_bearer_token(
    client: &Client,
    challenge: &BearerChallenge,
    creds: &BasicCredentials,
) -> Result<String, RegistryDigestError> {
    let mut url = Url::parse(&challenge.realm).map_err(|_| RegistryDigestError::ChallengeParse)?;
    {
        let mut query = url.query_pairs_mut();
        if let Some(service) = &challenge.service {
            query.append_pair("service", service);
        }
        if let Some(scope) = &challenge.scope {
            query.append_pair("scope", scope);
        }
    }

    let response = client
        .get(url)
        .basic_auth(&creds.username, Some(&creds.password))
        .send()
        .await
        .map_err(map_reqwest_error)?;

    if !response.status().is_success() {
        return Err(map_status_to_error(response.status()));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|_| RegistryDigestError::Json)?;
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("access_token").and_then(|v| v.as_str()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or(RegistryDigestError::BadResponse)?;

    Ok(token.to_string())
}

fn load_basic_credentials_for_registry(
    registry: &str,
) -> Result<BasicCredentials, RegistryDigestError> {
    let auths = load_containers_auth_json().map_err(|e| e)?;
    let registry_norm =
        normalize_registry_host(registry).ok_or(RegistryDigestError::AuthMissing)?;
    auths
        .get(&registry_norm)
        .cloned()
        .ok_or(RegistryDigestError::AuthMissing)
}

fn load_containers_auth_json() -> Result<HashMap<String, BasicCredentials>, RegistryDigestError> {
    let home = env::var("HOME").map_err(|_| RegistryDigestError::Io)?;
    let path: PathBuf = Path::new(&home).join(AUTH_JSON_REL_PATH);
    let raw = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Ok(HashMap::new());
            }
            return Err(RegistryDigestError::Io);
        }
    };

    let json: Value = serde_json::from_str(&raw).map_err(|_| RegistryDigestError::AuthParse)?;
    let mut out = HashMap::new();

    let Some(auths) = json.get("auths").and_then(|v| v.as_object()) else {
        return Ok(out);
    };

    for (key, entry) in auths.iter() {
        let Some(registry) = normalize_registry_host(key) else {
            continue;
        };
        let Some(obj) = entry.as_object() else {
            continue;
        };

        if let Some(auth) = obj.get("auth").and_then(|v| v.as_str()).map(|s| s.trim()) {
            if let Ok(decoded) = BASE64_STANDARD.decode(auth.as_bytes()) {
                if let Ok(decoded_str) = String::from_utf8(decoded) {
                    if let Some((user, pass)) = decoded_str.split_once(':') {
                        if !user.is_empty() {
                            out.insert(
                                registry,
                                BasicCredentials {
                                    username: user.to_string(),
                                    password: pass.to_string(),
                                },
                            );
                            continue;
                        }
                    }
                }
            }
        }

        let username = obj
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let password = obj
            .get("password")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if let (Some(username), Some(password)) = (username, password) {
            out.insert(
                registry,
                BasicCredentials {
                    username: username.to_string(),
                    password: password.to_string(),
                },
            );
        }
    }

    Ok(out)
}

fn normalize_registry_host(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        if let Ok(url) = Url::parse(trimmed) {
            if let Some(host) = url.host_str() {
                let host = host.to_ascii_lowercase();
                return Some(if let Some(port) = url.port() {
                    format!("{host}:{port}")
                } else {
                    host
                });
            }
        }
        let without_scheme = trimmed.splitn(2, "://").nth(1).unwrap_or(trimmed);
        let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
        return Some(host_port.to_ascii_lowercase());
    }

    Some(
        trimmed
            .split('/')
            .next()
            .unwrap_or(trimmed)
            .to_ascii_lowercase(),
    )
}

fn parse_image_ref(input: &str) -> Result<ParsedImageRef, RegistryDigestError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(RegistryDigestError::InvalidImage);
    }

    if raw.starts_with("http://") || raw.starts_with("https://") {
        let url = Url::parse(raw).map_err(|_| RegistryDigestError::InvalidImage)?;
        let scheme = url.scheme().to_string();
        let host = url
            .host_str()
            .ok_or(RegistryDigestError::InvalidImage)?
            .to_ascii_lowercase();
        let registry = if let Some(port) = url.port() {
            format!("{host}:{port}")
        } else {
            host
        };
        let path = url.path().trim_start_matches('/').to_string();
        let (repo, tag) = split_repo_tag(&path)?;
        let normalized_image = format!("{registry}/{repo}:{tag}");
        return Ok(ParsedImageRef {
            scheme,
            registry,
            repo,
            tag,
            normalized_image,
        });
    }

    let (registry_raw, rest) = raw
        .split_once('/')
        .ok_or(RegistryDigestError::InvalidImage)?;
    let registry =
        normalize_registry_host(registry_raw).ok_or(RegistryDigestError::InvalidImage)?;
    let (repo, tag) = split_repo_tag(rest)?;
    let normalized_image = format!("{registry}/{repo}:{tag}");
    Ok(ParsedImageRef {
        scheme: "https".to_string(),
        registry,
        repo,
        tag,
        normalized_image,
    })
}

fn split_repo_tag(path: &str) -> Result<(String, String), RegistryDigestError> {
    let trimmed = path.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(RegistryDigestError::InvalidImage);
    }

    let last_slash = trimmed.rfind('/').unwrap_or(0);
    let tag_sep = trimmed[last_slash..].rfind(':').map(|idx| idx + last_slash);
    let Some(tag_sep) = tag_sep else {
        return Err(RegistryDigestError::InvalidImage);
    };

    let repo = trimmed[..tag_sep].trim().to_string();
    let tag = trimmed[tag_sep + 1..].trim().to_string();
    if repo.is_empty() || tag.is_empty() {
        return Err(RegistryDigestError::InvalidImage);
    }
    Ok((repo, tag))
}

#[derive(Clone, Debug)]
struct CacheRow {
    image: String,
    digest: Option<String>,
    checked_at: i64,
    status: RegistryDigestStatus,
    error: Option<String>,
}

async fn get_cached_row(pool: &SqlitePool, image: &str) -> Result<Option<CacheRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT image, digest, checked_at, status, error FROM registry_digest_cache WHERE image = ?",
    )
    .bind(image)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };

    let image: String = row.get("image");
    let digest: Option<String> = row.get("digest");
    let checked_at: i64 = row.get("checked_at");
    let status_raw: String = row.get("status");
    let status = RegistryDigestStatus::from_db(&status_raw);
    let error: Option<String> = row.get("error");
    Ok(Some(CacheRow {
        image,
        digest,
        checked_at,
        status,
        error,
    }))
}

async fn upsert_cache_row(
    pool: &SqlitePool,
    image: &str,
    digest: Option<&str>,
    status: RegistryDigestStatus,
    error: Option<&str>,
) -> Result<RegistryDigestRecord, sqlx::Error> {
    let now = crate::current_unix_secs() as i64;

    sqlx::query(
        "INSERT INTO registry_digest_cache (image, digest, checked_at, status, error)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(image) DO UPDATE SET
           digest = excluded.digest,
           checked_at = excluded.checked_at,
           status = excluded.status,
           error = excluded.error",
    )
    .bind(image)
    .bind(digest)
    .bind(now)
    .bind(status.as_str())
    .bind(error)
    .execute(pool)
    .await?;

    Ok(RegistryDigestRecord {
        image: image.to_string(),
        digest: digest.map(|s| s.to_string()),
        checked_at: now,
        status,
        error: error.map(|s| s.to_string()),
        stale: status != RegistryDigestStatus::Ok,
        from_cache: false,
    })
}

fn is_expired(checked_at: i64, ttl_secs: u64) -> bool {
    let now = crate::current_unix_secs() as i64;
    let age = now.saturating_sub(checked_at).max(0) as u64;
    age > ttl_secs
}

fn compute_stale(checked_at: i64, ttl_secs: u64, status: RegistryDigestStatus) -> bool {
    is_expired(checked_at, ttl_secs) || status != RegistryDigestStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct HomeGuard {
        original: Option<String>,
    }

    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let original = env::var("HOME").ok();
            unsafe {
                env::set_var("HOME", path);
            }
            HomeGuard { original }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.take() {
                unsafe {
                    env::set_var("HOME", value);
                }
            }
        }
    }

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::MIGRATOR.run(&pool).await.unwrap();
        pool
    }

    #[derive(Clone)]
    enum AuthExpectation {
        None,
        Basic(String),
        Bearer(String),
    }

    #[derive(Clone)]
    struct Step {
        method: &'static str,
        path_prefix: &'static str,
        expect_auth: AuthExpectation,
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Option<String>,
    }

    struct MockServer {
        addr: String,
        hits: std::sync::Arc<AtomicUsize>,
    }

    impl MockServer {
        fn start<F>(make_steps: F) -> Self
        where
            F: FnOnce(String) -> Vec<Step>,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let addr_str = format!("127.0.0.1:{}", addr.port());
            let hits = std::sync::Arc::new(AtomicUsize::new(0));
            let hits_thread = hits.clone();
            let steps = std::sync::Arc::new(Mutex::new(make_steps(addr_str.clone())));

            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    hits_thread.fetch_add(1, Ordering::SeqCst);
                    let req = read_request(&mut stream);
                    let (method, path, headers) = parse_request(&req);

                    let (step, done) = {
                        let mut guard = steps.lock().unwrap();
                        if guard.is_empty() {
                            break;
                        }
                        let step = guard.remove(0);
                        let done = guard.is_empty();
                        (step, done)
                    };

                    assert_eq!(method, step.method);
                    assert!(
                        path.starts_with(step.path_prefix),
                        "path mismatch: got={path} expected_prefix={}",
                        step.path_prefix
                    );

                    match step.expect_auth {
                        AuthExpectation::None => {}
                        AuthExpectation::Basic(expected) => {
                            let got = headers.get("authorization").cloned().unwrap_or_default();
                            assert_eq!(got, format!("Basic {expected}"));
                        }
                        AuthExpectation::Bearer(expected) => {
                            let got = headers.get("authorization").cloned().unwrap_or_default();
                            assert_eq!(got, format!("Bearer {expected}"));
                        }
                    }

                    respond(
                        &mut stream,
                        step.status,
                        &step.headers,
                        step.body.as_deref(),
                    );

                    if done {
                        break;
                    }
                }
            });

            MockServer {
                addr: addr_str,
                hits,
            }
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    fn parse_request(raw: &str) -> (String, String, HashMap<String, String>) {
        let mut lines = raw.split("\r\n");
        let first = lines.next().unwrap_or_default();
        let mut first_parts = first.split_whitespace();
        let method = first_parts.next().unwrap_or_default().to_string();
        let path = first_parts.next().unwrap_or_default().to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        (method, path, headers)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if buf.len() > 64 * 1024 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    fn respond(
        stream: &mut TcpStream,
        status: u16,
        headers: &[(&str, String)],
        body: Option<&str>,
    ) {
        let body = body.unwrap_or("");
        let mut resp = String::new();
        resp.push_str(&format!("HTTP/1.1 {status} OK\r\n"));
        resp.push_str("Connection: close\r\n");
        resp.push_str(&format!("Content-Length: {}\r\n", body.as_bytes().len()));
        for (k, v) in headers {
            resp.push_str(k);
            resp.push_str(": ");
            resp.push_str(v);
            resp.push_str("\r\n");
        }
        resp.push_str("\r\n");
        resp.push_str(body);
        let _ = stream.write_all(resp.as_bytes());
    }

    fn write_auth_json(dir: &Path, registry: &str, username: &str, password: &str) {
        let auth = BASE64_STANDARD.encode(format!("{username}:{password}"));
        let path = dir.join(".config/containers");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("auth.json"),
            serde_json::json!({
                "auths": {
                    registry: {
                        "auth": auth
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_json_username_password_and_scheme_key_supported() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());

        let registry = "127.0.0.1:12345";
        let path = temp.path().join(".config/containers");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("auth.json"),
            serde_json::json!({
                "auths": {
                    format!("https://{registry}"): {
                        "username": "u1",
                        "password": "p1"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let creds = load_basic_credentials_for_registry(registry).unwrap();
        assert_eq!(creds.username, "u1");
        assert_eq!(creds.password, "p1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_digest_200_header_ok() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let pool = test_pool().await;

        let digest = "sha256:deadbeef";
        let server = MockServer::start(|_addr| {
            vec![Step {
                method: "HEAD",
                path_prefix: "/v2/repo/manifests/tag",
                expect_auth: AuthExpectation::None,
                status: 200,
                headers: vec![("Docker-Content-Digest", digest.to_string())],
                body: None,
            }]
        });

        let image = format!("http://{}/repo:tag", server.addr);
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Ok);
        assert_eq!(record.digest.as_deref(), Some(digest));
        assert!(!record.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_digest_401_bearer_challenge_then_ok() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let pool = test_pool().await;

        let username = "koha";
        let password = "secret";

        let digest = "sha256:beadfeed";
        let token_value = "t123";
        let server = MockServer::start(|addr| {
            write_auth_json(temp.path(), &addr, username, password);
            vec![
                Step {
                    method: "HEAD",
                    path_prefix: "/v2/repo/manifests/tag",
                    expect_auth: AuthExpectation::None,
                    status: 401,
                    headers: vec![(
                        "WWW-Authenticate",
                        format!(
                            "Bearer realm=\"http://{}/token\",service=\"mock\",scope=\"repository:repo:pull\"",
                            addr
                        ),
                    )],
                    body: None,
                },
                Step {
                    method: "GET",
                    path_prefix: "/token",
                    expect_auth: AuthExpectation::Basic(
                        BASE64_STANDARD.encode(format!("{username}:{password}")),
                    ),
                    status: 200,
                    headers: vec![("Content-Type", "application/json".to_string())],
                    body: Some(format!("{{\"token\":\"{token_value}\"}}")),
                },
                Step {
                    method: "HEAD",
                    path_prefix: "/v2/repo/manifests/tag",
                    expect_auth: AuthExpectation::Bearer(token_value.to_string()),
                    status: 200,
                    headers: vec![("Docker-Content-Digest", digest.to_string())],
                    body: None,
                },
            ]
        });

        let image = format!("http://{}/repo:tag", server.addr);
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Ok);
        assert_eq!(record.digest.as_deref(), Some(digest));
        assert!(!record.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_digest_missing_auth_returns_auth_missing() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let pool = test_pool().await;

        let server = MockServer::start(|_addr| {
            vec![Step {
                method: "HEAD",
                path_prefix: "/v2/repo/manifests/tag",
                expect_auth: AuthExpectation::None,
                status: 401,
                headers: vec![(
                    "WWW-Authenticate",
                    "Bearer realm=\"http://127.0.0.1/token\",service=\"mock\",scope=\"repository:repo:pull\""
                        .to_string(),
                )],
                body: None,
            }]
        });

        let image = format!("http://{}/repo:tag", server.addr);
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Error);
        assert_eq!(record.error.as_deref(), Some("auth-missing"));
        assert!(record.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_digest_200_without_digest_header_returns_digest_missing() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let pool = test_pool().await;

        let server = MockServer::start(|_addr| {
            vec![Step {
                method: "HEAD",
                path_prefix: "/v2/repo/manifests/tag",
                expect_auth: AuthExpectation::None,
                status: 200,
                headers: vec![],
                body: None,
            }]
        });

        let image = format!("http://{}/repo:tag", server.addr);
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Error);
        assert_eq!(record.error.as_deref(), Some("digest-missing"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cache_ttl_hit_expired_force_refresh_and_failure_fallback() {
        let _lock = env_lock();
        let temp = TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let pool = test_pool().await;

        let digest_old = "sha256:old";
        let digest_new = "sha256:new";
        let server = MockServer::start(|_addr| {
            vec![
                Step {
                    method: "HEAD",
                    path_prefix: "/v2/repo/manifests/tag",
                    expect_auth: AuthExpectation::None,
                    status: 200,
                    headers: vec![("Docker-Content-Digest", digest_new.to_string())],
                    body: None,
                },
                Step {
                    method: "HEAD",
                    path_prefix: "/v2/repo/manifests/tag",
                    expect_auth: AuthExpectation::None,
                    status: 200,
                    headers: vec![], // digest-missing
                    body: None,
                },
            ]
        });

        let image = format!("http://{}/repo:tag", server.addr);
        let parsed = parse_image_ref(&image).unwrap();

        // Insert a fresh cache row.
        let now = crate::current_unix_secs() as i64;
        sqlx::query(
            "INSERT INTO registry_digest_cache (image, digest, checked_at, status, error) VALUES (?, ?, ?, 'ok', NULL)",
        )
        .bind(&parsed.normalized_image)
        .bind(digest_old)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // TTL hit should not call server.
        let record = resolve_remote_manifest_digest(&pool, &image, 600, false).await;
        assert_eq!(record.status, RegistryDigestStatus::Ok);
        assert_eq!(record.digest.as_deref(), Some(digest_old));
        assert!(!record.stale);
        assert_eq!(server.hits(), 0);

        // Expired + non-force should return stale and still not call server.
        sqlx::query("UPDATE registry_digest_cache SET checked_at = ? WHERE image = ?")
            .bind(now - 601)
            .bind(&parsed.normalized_image)
            .execute(&pool)
            .await
            .unwrap();
        let record = resolve_remote_manifest_digest(&pool, &image, 600, false).await;
        assert_eq!(record.digest.as_deref(), Some(digest_old));
        assert!(record.stale);
        assert_eq!(server.hits(), 0);

        // Force refresh succeeds and updates digest.
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Ok);
        assert_eq!(record.digest.as_deref(), Some(digest_new));
        assert!(!record.stale);
        assert_eq!(server.hits(), 1);

        // Force refresh failure returns old digest + stale + error, and error is sanitized.
        sqlx::query("UPDATE registry_digest_cache SET checked_at = ? WHERE image = ?")
            .bind(now - 601)
            .bind(&parsed.normalized_image)
            .execute(&pool)
            .await
            .unwrap();
        let record = resolve_remote_manifest_digest(&pool, &image, 600, true).await;
        assert_eq!(record.status, RegistryDigestStatus::Error);
        assert_eq!(record.digest.as_deref(), Some(digest_new));
        assert!(record.stale);
        assert_eq!(record.error.as_deref(), Some("digest-missing"));
        assert_eq!(server.hits(), 2);

        let db_error: Option<String> =
            sqlx::query_scalar("SELECT error FROM registry_digest_cache WHERE image = ?")
                .bind(&parsed.normalized_image)
                .fetch_one(&pool)
                .await
                .unwrap();
        let db_error = db_error.unwrap_or_default();
        for forbidden in ["Authorization", "koha", "secret", "t123"] {
            assert!(
                !db_error.contains(forbidden),
                "error field should not contain sensitive substring: {forbidden}"
            );
        }
    }
}

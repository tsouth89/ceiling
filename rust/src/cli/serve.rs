//! Local HTTP server for scriptable usage/cost JSON.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::usage::ProviderSelection;
use crate::core::{
    ConfiguredAccounts, FetchContext, ProviderId, SourceMode, UsageSnapshot, instantiate_provider,
};
use crate::cost_scanner::CostScanner;

const UNAUTHENTICATED_ERROR: &str = "unauthorized";
const PROVIDER_FAILURE: &str = "provider request failed";
const MAX_HEADER_BYTES: usize = 8192;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    /// Local HTTP port
    #[arg(long, default_value = "8080")]
    pub port: u16,

    /// Response cache TTL in seconds
    #[arg(long = "refresh-interval", default_value = "60")]
    pub refresh_interval: u64,

    /// Allow any local process to read usage without a bearer token.
    /// Existing scripts can keep working; this is not the default.
    #[arg(long)]
    pub allow_unauthenticated: bool,

    /// Include account identity and raw provider errors in responses.
    #[arg(long)]
    pub include_identity: bool,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let expected_token = if args.allow_unauthenticated {
        None
    } else {
        let loaded = load_or_create_serve_token()?;
        eprintln!("Ceiling server token stored at {}", loaded.path.display());
        if loaded.created {
            eprintln!("Authorization: Bearer {}", loaded.token);
        }
        Some(loaded.token)
    };
    let include_identity = args.include_identity;
    let listener = TcpListener::bind(("127.0.0.1", args.port)).await?;
    eprintln!("Ceiling server listening on http://127.0.0.1:{}", args.port);

    loop {
        let (stream, _) = listener.accept().await?;
        let expected_token = expected_token.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_client(stream, expected_token.as_deref(), include_identity).await
            {
                tracing::debug!("serve client error: {error}");
            }
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
    expected_token: Option<&str>,
    include_identity: bool,
) -> anyhow::Result<()> {
    let request = read_http_headers(&mut stream).await?;
    let response = match parse_request(&request) {
        Ok(request) => route_request(&request, expected_token, include_identity).await,
        Err(status) => json_response(status, serde_json::json!({ "error": "bad request" })),
    };
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn route_request(
    request: &ServeRequest,
    expected_token: Option<&str>,
    include_identity: bool,
) -> String {
    if request.method != "GET" {
        return json_response(405, serde_json::json!({ "error": "method not allowed" }));
    }
    if !allowed_host(&request.host) {
        return json_response(403, serde_json::json!({ "error": "forbidden host" }));
    }
    if request.path != "/health" && !request_is_authorized(request, expected_token) {
        return json_response(401, serde_json::json!({ "error": UNAUTHENTICATED_ERROR }));
    }

    match request.path.as_str() {
        "/health" => json_response(
            200,
            serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }),
        ),
        "/usage" => {
            usage_response(
                request.query.get("provider").map(String::as_str),
                include_identity,
            )
            .await
        }
        "/cost" => cost_response(request.query.get("provider").map(String::as_str)).await,
        _ => json_response(404, serde_json::json!({ "error": "not found" })),
    }
}

async fn usage_response(provider: Option<&str>, include_identity: bool) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, serde_json::json!({ "error": error.to_string() }));
        }
    };
    let ctx = FetchContext {
        source_mode: SourceMode::Auto,
        include_credits: true,
        web_timeout: 60,
        verbose: false,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: None,
        api_region: None,
        gateway_url: None,
        account_config_dir: None,
    };
    let accounts = ConfiguredAccounts::load();

    let mut results = Vec::new();
    for provider_id in selection.as_list() {
        let provider = instantiate_provider(provider_id);
        match provider
            .fetch_usage(&ctx.clone().for_account(provider_id, &accounts))
            .await
        {
            Ok(result) => results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "source": result.source_label,
                "usage": public_usage(result.usage, include_identity),
                "cost": result.cost,
            })),
            Err(error) => results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "error": public_error(error.to_string(), include_identity),
            })),
        }
    }
    json_response(200, serde_json::Value::Array(results))
}

async fn cost_response(provider: Option<&str>) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, serde_json::json!({ "error": error.to_string() }));
        }
    };
    let scanner = CostScanner::new(30);
    json_response(
        200,
        serde_json::Value::Array(cost_payloads(&scanner, &selection.as_list())),
    )
}

fn cost_payloads(scanner: &CostScanner, providers: &[ProviderId]) -> Vec<serde_json::Value> {
    providers
        .iter()
        .copied()
        .map(|provider_id| match scanner.scan_provider(provider_id) {
            Some(summary) => serde_json::json!({
                "provider": provider_id.cli_name(),
                "supported": true,
                "days_scanned": 30,
                "cost": {
                    "total_usd": summary.total_cost_usd,
                    "currency": "USD"
                },
                "tokens": {
                    "input": summary.input_tokens,
                    "output": summary.output_tokens,
                    "cached": summary.cached_tokens
                },
                "sessions_count": summary.sessions_count,
                "by_model": summary.by_model,
            }),
            None => serde_json::json!({
                "provider": provider_id.cli_name(),
                "supported": false,
                "error": "Local cost scanning not available for this provider"
            }),
        })
        .collect()
}

#[derive(Debug)]
struct ServeRequest {
    method: String,
    path: String,
    host: String,
    authorization: Option<String>,
    query: std::collections::HashMap<String, String>,
}

fn parse_request(raw: &str) -> Result<ServeRequest, u16> {
    let mut lines = raw.split("\r\n");
    let first = lines.next().ok_or(400_u16)?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or(400_u16)?.to_uppercase();
    let target = parts.next().ok_or(400_u16)?;
    if parts.next().is_none() || !target.starts_with('/') {
        return Err(400);
    }

    let mut hosts = Vec::new();
    let mut authorization = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(400);
        };
        if name.trim().eq_ignore_ascii_case("host") {
            hosts.push(value.trim().to_string());
        } else if name.trim().eq_ignore_ascii_case("authorization") {
            if authorization.is_some() {
                return Err(400);
            }
            authorization = Some(value.trim().to_string());
        }
    }
    if hosts.len() != 1 {
        return Err(400);
    }

    let (path, query) = parse_target(target);
    Ok(ServeRequest {
        method,
        path,
        host: hosts.remove(0),
        authorization,
        query,
    })
}

fn parse_target(target: &str) -> (String, std::collections::HashMap<String, String>) {
    let Some((path, query_string)) = target.split_once('?') else {
        return (target.to_string(), Default::default());
    };
    let query = query_string
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((url_decode(key), url_decode(value)))
        })
        .collect();
    (path.to_string(), query)
}

fn allowed_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed.contains(',') {
        return false;
    }
    let without_port = if let Some(rest) = trimmed.strip_prefix('[') {
        let Some((addr, port)) = rest.split_once(']') else {
            return false;
        };
        if !port.is_empty() && !valid_port_suffix(port) {
            return false;
        }
        format!("[{addr}]")
    } else {
        let segments: Vec<_> = trimmed.split(':').collect();
        match segments.as_slice() {
            [host] => host.to_string(),
            [host, port] if valid_port(port) => host.to_string(),
            _ => return false,
        }
    };
    matches!(
        without_port.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "localhost." | "[::1]"
    )
}

fn valid_port_suffix(raw: &str) -> bool {
    raw.is_empty() || raw.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(raw: &str) -> bool {
    raw.parse::<u16>().is_ok_and(|port| port > 0)
}

fn url_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut bytes = raw.as_bytes().iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'+' {
            out.push(' ');
        } else if byte == b'%' {
            let hi = bytes.next();
            let lo = bytes.next();
            if let (Some(hi), Some(lo)) = (hi, lo)
                && let Ok(value) =
                    u8::from_str_radix(std::str::from_utf8(&[hi, lo]).unwrap_or_default(), 16)
            {
                out.push(value as char);
            }
        } else {
            out.push(byte as char);
        }
    }
    out
}

fn public_usage(mut usage: UsageSnapshot, include_identity: bool) -> UsageSnapshot {
    if !include_identity {
        usage.account_email = None;
        usage.account_organization = None;
        usage.login_method = None;
    }
    usage
}

fn public_error(error: String, include_identity: bool) -> String {
    if include_identity {
        crate::core::SecretRedactor::redact(&error)
    } else {
        PROVIDER_FAILURE.to_string()
    }
}

fn request_is_authorized(request: &ServeRequest, expected_token: Option<&str>) -> bool {
    let Some(expected) = expected_token else {
        return true;
    };
    let Some(header) = request.authorization.as_deref() else {
        return false;
    };
    bearer_matches(header, expected)
}

fn bearer_matches(header: &str, expected: &str) -> bool {
    let Some(provided) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    else {
        return false;
    };
    tokens_equal(provided.trim(), expected)
}

fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

struct ServeToken {
    token: String,
    path: PathBuf,
    created: bool,
}

async fn read_http_headers(stream: &mut TcpStream) -> io::Result<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let n = tokio::time::timeout(HEADER_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "timed out reading request headers")
            })??;
        if n == 0 {
            break;
        }
        if buffer.len().saturating_add(n) > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
        buffer.extend_from_slice(&chunk[..n]);
        if headers_complete(&buffer) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn headers_complete(buffer: &[u8]) -> bool {
    buffer.windows(4).any(|window| window == b"\r\n\r\n")
}

fn serve_token_path() -> io::Result<PathBuf> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user config directory is unavailable",
        )
    })?;
    Ok(config_dir.join("Ceiling").join("serve.token"))
}

fn load_or_create_serve_token() -> io::Result<ServeToken> {
    load_or_create_serve_token_at(serve_token_path()?)
}

fn load_or_create_serve_token_at(path: PathBuf) -> io::Result<ServeToken> {
    if path.exists() {
        match read_existing_serve_token(path.clone()) {
            Ok(token) => return Ok(token),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let _ = std::fs::remove_file(&path);
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match create_new_serve_token(&path) {
        Ok(token) => Ok(ServeToken {
            token,
            path,
            created: true,
        }),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            read_existing_serve_token(path)
        }
        Err(error) => Err(error),
    }
}

fn read_existing_serve_token(path: PathBuf) -> io::Result<ServeToken> {
    let token = std::fs::read_to_string(&path)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "serve token file is empty",
        ));
    }
    protect_token_file(&path)?;
    validate_existing_token_file(&path)?;
    Ok(ServeToken {
        token: token.to_string(),
        path,
        created: false,
    })
}

fn create_new_serve_token(path: &Path) -> io::Result<String> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let write_result = (|| -> io::Result<()> {
        protect_token_file(path)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(token)
}

fn validate_existing_token_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "serve token file is readable by other users",
            ));
        }
    }
    let _ = path;
    Ok(())
}

fn protect_token_file(path: &std::path::Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::windows_security::restrict_path_to_current_user(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        Ok(())
    }
}

fn json_response(status: u16, payload: serde_json::Value) -> String {
    let body = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_hosts() {
        assert!(allowed_host("127.0.0.1:8080"));
        assert!(allowed_host("localhost"));
        assert!(allowed_host("[::1]:8080"));
        assert!(!allowed_host("example.com"));
        assert!(!allowed_host("127.0.0.1, example.com"));
    }

    #[test]
    fn parses_usage_route_provider_query() {
        let request =
            parse_request("GET /usage?provider=deepseek HTTP/1.1\r\nHost: localhost:8080\r\n\r\n")
                .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/usage");
        assert_eq!(request.query.get("provider"), Some(&"deepseek".to_string()));
        assert_eq!(request.authorization, None);
    }

    #[test]
    fn rejects_missing_or_wrong_bearer_token() {
        let request = parse_request("GET /usage HTTP/1.1\r\nHost: localhost:8080\r\n\r\n").unwrap();
        assert!(!request_is_authorized(&request, Some("secret-token")));

        let wrong = parse_request(
            "GET /usage HTTP/1.1\r\nHost: localhost:8080\r\nAuthorization: Bearer other\r\n\r\n",
        )
        .unwrap();
        assert!(!request_is_authorized(&wrong, Some("secret-token")));
    }

    #[test]
    fn accepts_matching_bearer_token() {
        let request = parse_request(
            "GET /usage HTTP/1.1\r\nHost: localhost:8080\r\nAuthorization: Bearer secret-token\r\n\r\n",
        )
        .unwrap();
        assert!(request_is_authorized(&request, Some("secret-token")));
        assert!(request_is_authorized(&request, None));
    }

    #[test]
    fn redacts_identity_and_provider_errors_by_default() {
        let mut usage = UsageSnapshot::new(crate::core::RateWindow::new(10.0));
        usage.account_email = Some("user@example.com".into());
        usage.account_organization = Some("Acme".into());
        usage.login_method = Some("oauth".into());

        let redacted = public_usage(usage.clone(), false);
        assert_eq!(redacted.account_email, None);
        assert_eq!(redacted.account_organization, None);
        assert_eq!(redacted.login_method, None);

        let full = public_usage(usage, true);
        assert_eq!(full.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(
            public_error("signed URL https://x/y?token=abc".into(), false),
            PROVIDER_FAILURE
        );
        assert!(!public_error("Bearer abcdef".into(), true).contains("abcdef"));
    }

    #[test]
    fn persists_and_reuses_a_serve_token() {
        let dir = std::env::temp_dir().join(format!(
            "ceiling-serve-token-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("serve.token");
        let first = load_or_create_serve_token_at(path.clone()).unwrap();
        assert!(first.created);
        let second = load_or_create_serve_token_at(path.clone()).unwrap();
        assert!(!second.created);
        assert_eq!(first.token, second.token);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), first.token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replaces_an_empty_serve_token_file() {
        let dir = std::env::temp_dir().join(format!(
            "ceiling-serve-token-empty-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("serve.token");
        std::fs::write(&path, "").unwrap();
        let loaded = load_or_create_serve_token_at(path.clone()).unwrap();
        assert!(loaded.created);
        assert!(!loaded.token.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), loaded.token);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn headers_complete_requires_the_terminator() {
        assert!(!headers_complete(b"GET /usage HTTP/1.1\r\nHost: localhost"));
        assert!(headers_complete(
            b"GET /usage HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret\r\n\r\n"
        ));
    }

    /// SBS-934: GET /cost used the same Codex/Claude-only match as the CLI
    /// and marked Grok unsupported even when CostScanner can scan it.
    #[test]
    fn cost_payloads_mark_grok_supported_and_cursor_not() {
        let scanner = CostScanner::new(7);
        let payloads = cost_payloads(&scanner, &[ProviderId::Grok, ProviderId::Cursor]);
        assert_eq!(payloads[0]["provider"], "grok");
        assert_eq!(payloads[0]["supported"], true);
        assert!(payloads[0].get("error").is_none());
        assert_eq!(payloads[1]["provider"], "cursor");
        assert_eq!(payloads[1]["supported"], false);
    }

    #[tokio::test]
    async fn health_bypasses_auth_and_other_paths_require_token() {
        let request =
            parse_request("GET /health HTTP/1.1\r\nHost: localhost:8080\r\n\r\n").unwrap();
        assert!(
            route_request(&request, Some("secret-token"), false)
                .await
                .starts_with("HTTP/1.1 200")
        );

        let denied = parse_request("GET /cost HTTP/1.1\r\nHost: localhost:8080\r\n\r\n").unwrap();
        assert!(
            route_request(&denied, Some("secret-token"), false)
                .await
                .starts_with("HTTP/1.1 401")
        );
    }
}

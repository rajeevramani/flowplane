//! `flowplane dashboard` (fpv2-03m.2): a read-only, loopback-only presentation server.
//!
//! Trust boundary (design: releases/3.1.0 ui-f1, "Security posture of the local server"):
//! the server binds 127.0.0.1 on an ephemeral port, every route lives under a per-launch
//! 128-bit CSPRNG nonce path prefix (no route exists outside it), only GET handlers are
//! registered, `Host`/`Origin` are validated, and every response carries no-store /
//! CSP-self / no-referrer / frame-deny headers. The bearer token stays in process memory;
//! nothing in this module writes it to a response or a log.

mod data;
mod filters_inventory;
mod joins;
mod orphans;
mod ratelimits;
mod resources;
mod secrets_panel;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::client::RestClient;
use super::config::GlobalOptions;

/// Container-profile flags (fpv2-m4u.1, design: releases/3.1.0 ui-f3). The native
/// profile (no flags) is unchanged: loopback ephemeral bind, browser auto-open, no file.
#[derive(Debug, Default, clap::Args)]
pub struct DashboardOptions {
    /// Bind address (default 127.0.0.1 on an ephemeral port). Off-loopback binds print a
    /// prominent warning; the per-launch URL nonce stays mandatory on every route.
    #[arg(long, value_name = "ADDR:PORT")]
    pub listen: Option<SocketAddr>,
    /// Do not open a browser (headless/container use).
    #[arg(long)]
    pub no_open: bool,
    /// Write the dashboard URL to this file once the server is ready (bound + first
    /// successful upstream fetch). The file is the container readiness contract: any
    /// pre-existing file is deleted before binding, the write is atomic, and an
    /// unwritable path is fatal.
    #[arg(long, value_name = "PATH")]
    pub url_file: Option<PathBuf>,
}

/// Pinned, vendored htmx (2.0.4, sha256 e209dda5c8235479f3166defc7750e1dbcd5a5c1808b7792fc2e6733768fb447).
/// Served same-origin so the CSP stays `default-src 'self'` and the page works offline.
const HTMX_JS: &[u8] = include_bytes!("assets/htmx.min.js");
const DASHBOARD_CSS: &str = include_str!("assets/dashboard.css");
/// Same-origin hover-highlight helper for the topology view (CSP `default-src 'self'`
/// forbids inline script, so this ships as a served asset).
const RESOURCES_JS: &str = include_str!("assets/resources.js");

/// Every route the dashboard serves, WITHOUT the nonce prefix. The router is built from
/// this table and from nothing else, so a route that skips the nonce prefix cannot exist;
/// tests iterate it to prove each route class 404s without the nonce.
pub(crate) const ROUTE_PATHS: &[&str] = &[
    "/",
    "/resources",
    "/partials/overview",
    "/partials/resources/topology",
    "/partials/resources/clusters",
    "/partials/resources/route-configs",
    "/partials/resources/listeners",
    "/partials/resources/rate-limits",
    "/partials/resources/rate-limit-policies",
    "/partials/resources/filters",
    "/partials/resources/secrets",
    "/partials/resources/orphans",
    "/assets/htmx.min.js",
    "/assets/dashboard.css",
    "/assets/resources.js",
];

pub(crate) struct DashState {
    /// Hex-encoded per-launch 128-bit CSPRNG nonce; the only valid path prefix.
    nonce: String,
    /// Port actually bound, for Host/Origin validation.
    port: u16,
    /// Authenticated REST client (bearer + org header); the data path's only exit.
    client: RestClient,
    /// Resolved team the dashboard renders.
    team: String,
}

/// The configured team is interpolated into the two allowlisted upstream paths as ONE
/// path segment. Constrain it to URL-safe name/UUID characters so a hostile config
/// value (`/`, `?`, `#`, `%`, dot-segments) cannot change which paths are requested —
/// the fixed two-GET allowlist is a design invariant. CP team names are lowercase
/// alphanumerics with single hyphens; team UUIDs are hex + hyphens; both fit.
fn validate_team_segment(team: &str) -> Result<()> {
    let valid = !team.is_empty()
        && team.len() <= 100
        && team.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if valid {
        Ok(())
    } else {
        anyhow::bail!("invalid team name for dashboard: must be 1-100 letters, digits, or hyphens")
    }
}

/// 128-bit nonce from the OS CSPRNG, hex-encoded (32 chars).
pub(crate) fn generate_nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("generate dashboard nonce")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) async fn run(global: GlobalOptions, options: DashboardOptions) -> Result<()> {
    let client = RestClient::new(global)?;
    // Fail exactly like every CLI command when no team is resolvable — before binding.
    let team = client.team(None)?;
    validate_team_segment(&team)?;
    let nonce = generate_nonce()?;

    // url-file lifecycle, BEFORE binding (design: a stale nonce from a prior launch must
    // never satisfy discovery or the container healthcheck): delete any pre-existing
    // file and prove the path writable. With --url-file the file IS the readiness
    // contract, so an unusable path is fatal — never warn-and-continue.
    if let Some(path) = &options.url_file {
        prepare_url_file(path)?;
    }

    let bind_addr = options
        .listen
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
    if !bind_addr.ip().is_loopback() {
        eprintln!(
            "WARNING: --listen {bind_addr} binds off-loopback: the dashboard is reachable \
             by other hosts/containers on that network. Every route still requires the \
             per-launch URL nonce, and Host/Origin must be loopback with the bound port."
        );
    }
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("bind dashboard to {bind_addr}"))?;
    let addr = listener.local_addr().context("read bound address")?;
    let state = Arc::new(DashState {
        nonce,
        port: addr.port(),
        client,
        team,
    });
    let app = build_router(state.clone());

    // The URL is NEVER derived from the bind address (design-review pass 1): an
    // off-loopback bind still yields a loopback URL, because the Host/Origin allowlist
    // accepts only 127.0.0.1/localhost with the bound port, and container publishes map
    // the same port number on the host.
    let url = dashboard_url(addr.port(), &state.nonce);
    // Always print the URL (headless hosts and tests rely on it), then try the platform
    // opener; a missing/failing opener is non-fatal by design.
    println!("Dashboard running at {url} (Ctrl-C to stop)");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    if !options.no_open && std::env::var_os("FLOWPLANE_DASHBOARD_NO_BROWSER").is_none() {
        open_browser(&url);
    }

    if let Some(path) = options.url_file.clone() {
        tokio::spawn(readiness_writer(state.clone(), path, url));
    }

    axum::serve(listener, app)
        .await
        .context("serve dashboard")?;
    Ok(())
}

/// Loopback URL for the bound port — the single derivation point (never the bind address).
fn dashboard_url(port: u16, nonce: &str) -> String {
    format!("http://127.0.0.1:{port}/{nonce}/")
}

/// Pre-bind url-file lifecycle: remove any stale file, then prove — with sibling probe
/// files — the exact capabilities the later atomic publish needs (create, rename, remove
/// in the target directory). Every failure, including cleanup, is fatal by contract: a
/// directory that cannot rename or remove would break the publish after binding.
fn prepare_url_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("delete stale --url-file {}", path.display()));
        }
    }
    let probe = sibling_path(path, "probe");
    let probe_renamed = sibling_path(path, "probe2");
    std::fs::write(&probe, b"probe")
        .with_context(|| format!("--url-file path {} is not writable", path.display()))?;
    std::fs::rename(&probe, &probe_renamed).with_context(|| {
        format!(
            "--url-file path {} is not writable (rename failed)",
            path.display()
        )
    })?;
    std::fs::remove_file(&probe_renamed).with_context(|| {
        format!(
            "--url-file path {} is not writable (cleanup failed)",
            path.display()
        )
    })?;
    Ok(())
}

/// Atomic publish: write a temp sibling, then rename over the target (same directory, so
/// the rename cannot cross filesystems). Readers never observe a partial file.
fn write_url_file_atomic(path: &Path, url: &str) -> Result<()> {
    let tmp = sibling_path(path, "tmp");
    std::fs::write(&tmp, format!("{url}\n").as_bytes())
        .with_context(|| format!("write temp url-file {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename url-file into place at {}", path.display()))?;
    Ok(())
}

/// `<path>.<tag>-<pid>` beside the target, so probe/temp files stay on the same
/// filesystem and cannot collide across concurrent dashboards.
fn sibling_path(path: &Path, tag: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{tag}-{}", std::process::id()));
    PathBuf::from(name)
}

/// Container readiness: retry the allowlisted stats read until the upstream answers once,
/// then atomically publish the URL file. The file appearing is the healthcheck signal, so
/// a write failure here exits the process (no healthy-but-fileless state can exist, and
/// the inverse — fileless-but-running — must not persist silently either).
async fn readiness_writer(state: Arc<DashState>, path: PathBuf, url: String) {
    let probe = format!("/api/v1/teams/{}/stats/overview", state.team);
    let mut attempts: u32 = 0;
    loop {
        match state.client.get_json(&probe).await {
            Ok(_) => break,
            Err(err) => {
                if attempts.is_multiple_of(30) {
                    eprintln!("dashboard upstream not ready yet ({err}); retrying");
                }
                attempts = attempts.saturating_add(1);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
    if let Err(err) = write_url_file_atomic(&path, &url) {
        eprintln!(
            "FATAL: could not write --url-file {}: {err:#}",
            path.display()
        );
        std::process::exit(1);
    }
    eprintln!("dashboard URL written to {}", path.display());
}

/// Best-effort browser launch via the platform opener; failure only prints a note.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    if let Err(err) = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("could not open a browser ({err}); open the URL above manually");
    }
}

/// Build the full router: ROUTE_PATHS registered under the nonce prefix, a 404 fallback
/// for everything else, and the Host/Origin + security-header middleware around it all.
pub(crate) fn build_router(state: Arc<DashState>) -> Router {
    let mut router = Router::new();
    for path in ROUTE_PATHS {
        let full = format!("/{}{}", state.nonce, path);
        // Trailing slash form: "/{nonce}/" for the page, "/{nonce}/assets/…" for assets.
        router = router.route(
            &full,
            match *path {
                "/" => get(overview),
                "/resources" => get(resources_page),
                "/partials/overview" => get(overview_partial),
                "/partials/resources/topology" => get(resources_topology_partial),
                "/partials/resources/clusters" => get(resources_clusters_partial),
                "/partials/resources/route-configs" => get(resources_route_configs_partial),
                "/partials/resources/listeners" => get(resources_listeners_partial),
                "/partials/resources/rate-limits" => get(resources_rate_limits_partial),
                "/partials/resources/rate-limit-policies" => {
                    get(resources_rate_limit_policies_partial)
                }
                "/partials/resources/filters" => get(resources_filters_partial),
                "/partials/resources/secrets" => get(resources_secrets_partial),
                "/partials/resources/orphans" => get(resources_orphans_partial),
                "/assets/htmx.min.js" => get(htmx_js),
                "/assets/dashboard.css" => get(dashboard_css),
                "/assets/resources.js" => get(resources_js),
                other => unreachable!("unrouted dashboard path {other}"),
            },
        );
    }
    router
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Request guard + response hardening, in contract order (design AC 2/3):
/// 1. a path outside `/<nonce>/<ROUTE_PATHS entry>` → 404, regardless of anything else;
/// 2. any method other than GET on a real route → 405 (axum's `get()` would otherwise
///    also serve HEAD; the design says GET only);
/// 3. a foreign `Host` or `Origin` → 403.
///
/// Every response — success or error — carries the security headers.
async fn guard(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Route knowledge derives from the same ROUTE_PATHS table the router is built from,
    // so the two cannot drift.
    let known_route = request
        .uri()
        .path()
        .strip_prefix(&format!("/{}", state.nonce))
        .map(|rest| ROUTE_PATHS.contains(&rest))
        .unwrap_or(false);
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| host_allowed(host, state.port))
        // No Host at all is not a browser; reject rather than guess.
        .unwrap_or(false);
    let origin_ok = match request.headers().get(header::ORIGIN) {
        // Headless/no-origin requests are fine (design: absent Origin allowed).
        None => true,
        Some(v) => v
            .to_str()
            .map(|origin| origin_allowed(origin, state.port))
            .unwrap_or(false),
    };
    let mut response = if !known_route {
        StatusCode::NOT_FOUND.into_response()
    } else if request.method() != axum::http::Method::GET {
        StatusCode::METHOD_NOT_ALLOWED.into_response()
    } else if !(host_ok && origin_ok) {
        StatusCode::FORBIDDEN.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// `Host` must be loopback by NAME (127.0.0.1 or localhost), with the port — when present —
/// equal to the bound port. Anything else (a DNS-rebinding host, another local service's
/// port) is rejected.
fn host_allowed(host: &str, port: u16) -> bool {
    let (name, host_port) = match host.rsplit_once(':') {
        Some((name, port_str)) => match port_str.parse::<u16>() {
            Ok(p) => (name, Some(p)),
            Err(_) => return false,
        },
        None => (host, None),
    };
    let name_ok = name == "127.0.0.1" || name == "localhost";
    name_ok && host_port.is_none_or(|p| p == port)
}

/// A present `Origin` must be exactly this server's own origin.
fn origin_allowed(origin: &str, port: u16) -> bool {
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

#[derive(askama::Template)]
#[template(path = "dashboard/overview.html")]
struct OverviewShell<'a> {
    nonce: &'a str,
    team: &'a str,
}

async fn overview(axum::extract::State(state): axum::extract::State<Arc<DashState>>) -> Response {
    let shell = OverviewShell {
        nonce: &state.nonce,
        team: &state.team,
    };
    match askama::Template::render(&shell) {
        Ok(html) => Html(html).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(askama::Template)]
#[template(path = "dashboard/overview_panel.html")]
struct OverviewPanel {
    data: data::OverviewData,
}

#[derive(askama::Template)]
#[template(path = "dashboard/auth_expired.html")]
struct AuthExpiredBanner;

/// The htmx-polled partial (design AC 1/5): both allowlisted reads, per-panel states.
/// Upstream 401 → the re-login banner with HTTP 286, which tells htmx to STOP polling.
async fn overview_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    match data::fetch(&state.client, &state.team, chrono::Utc::now()).await {
        Ok(overview) => {
            let panel = OverviewPanel { data: overview };
            match askama::Template::render(&panel) {
                Ok(html) => Html(html).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(data::AuthExpired) => {
            // 286 is htmx's "stop polling" status; the swap still happens.
            let status = StatusCode::from_u16(286).unwrap_or(StatusCode::OK);
            match askama::Template::render(&AuthExpiredBanner) {
                Ok(html) => (status, Html(html)).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources.html")]
struct ResourcesShell<'a> {
    nonce: &'a str,
    team: &'a str,
}

/// The Resources page shell (fpv2-cxw.1). Renders NO data itself: each panel is an
/// htmx-lazy `<details>` panel that fetches its partial on first open (toggle), so an unopened panel
/// issues no upstream requests.
async fn resources_page(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let shell = ResourcesShell {
        nonce: &state.nonce,
        team: &state.team,
    };
    match askama::Template::render(&shell) {
        Ok(html) => Html(html).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_clusters.html")]
struct ClustersPanel {
    panel: data::Panel<resources::Table<resources::ClusterRow>>,
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_route_configs.html")]
struct RouteConfigsPanel {
    panel: data::Panel<resources::Table<resources::RouteConfigRow>>,
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_listeners.html")]
struct ListenersPanel {
    panel: data::Panel<resources::Table<resources::ListenerRow>>,
}

/// Render one resources partial: 401 → the shared re-login banner with htmx's 286
/// stop status (same seam as the overview partial), retargeted at the whole
/// `#resources` main so the expired-session state is dashboard-global — every panel
/// is replaced, not just the one that happened to fetch; anything else → the panel
/// state.
fn render_resources_panel<T: askama::Template>(result: Result<T, data::AuthExpired>) -> Response {
    match result {
        Ok(panel) => match askama::Template::render(&panel) {
            Ok(html) => Html(html).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(data::AuthExpired) => {
            let status = StatusCode::from_u16(286).unwrap_or(StatusCode::OK);
            match askama::Template::render(&AuthExpiredBanner) {
                Ok(html) => (
                    status,
                    [("HX-Retarget", "#resources"), ("HX-Reswap", "innerHTML")],
                    Html(html),
                )
                    .into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_topology.html")]
struct TopologyPanelTemplate {
    panel: resources::TopologyPanel,
}

async fn resources_topology_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = resources::fetch_topology(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| TopologyPanelTemplate { panel });
    render_resources_panel(result)
}

async fn resources_clusters_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = resources::fetch_clusters(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| ClustersPanel { panel });
    render_resources_panel(result)
}

async fn resources_route_configs_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = resources::fetch_route_configs(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| RouteConfigsPanel { panel });
    render_resources_panel(result)
}

async fn resources_listeners_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = resources::fetch_listeners(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| ListenersPanel { panel });
    render_resources_panel(result)
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_rate_limits.html")]
struct RateLimitsPanelTemplate<'a> {
    nonce: &'a str,
    panel: data::Panel<ratelimits::RateLimitsData>,
}

async fn resources_rate_limits_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let nonce = state.nonce.clone();
    let result = ratelimits::fetch_rate_limits(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| RateLimitsPanelTemplate {
            nonce: &nonce,
            panel,
        });
    render_resources_panel(result)
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_rate_limit_policies.html")]
struct PoliciesPanelTemplate {
    panel: data::Panel<resources::Table<ratelimits::PolicyRow>>,
}

/// Extract and percent-decode the `domain` value from a raw query string. Decoding
/// here (not axum's Query) keeps the route registrable without the parameter — the
/// nonce-route contract keeps every registered route serving 200.
fn domain_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    let raw = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("domain="))?;
    // Percent-decode; '+' is NOT space here (the URL was built by encode_segment).
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = char::from(hex[0]).to_digit(16)?;
            let lo = char::from(hex[1]).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Lazy per-domain policies list. The domain arrives as a query value; mirror the
/// CP's domain-name bounds before using it, then the fetch layer percent-encodes it
/// into the upstream path.
async fn resources_rate_limit_policies_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
    request: Request<Body>,
) -> Response {
    // A missing/invalid domain renders a 200 panel-state message (the nonce-route
    // contract keeps every registered route serving 200; htmx swaps the message in).
    let invalid =
        || Html("<p class=\"unavailable\">missing or invalid domain parameter</p>").into_response();
    let Some(domain) = domain_from_query(request.uri().query()) else {
        return invalid();
    };
    // The canonical domain-name contract (1–253 CHARS, not bytes, no control chars) —
    // the same validator the CP enforces, so a legal Unicode domain near the limit is
    // never rejected here while its row rendered fine in the sweep.
    if fp_domain::rate_limit::validate_rate_limit_domain_name(&domain).is_err() {
        return invalid();
    }
    let result = ratelimits::fetch_policies(&state.client, &state.team, &domain)
        .await
        .map(|panel| PoliciesPanelTemplate { panel });
    render_resources_panel(result)
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_filters.html")]
struct FiltersPanelTemplate {
    panel: filters_inventory::FiltersPanel,
}

async fn resources_filters_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = filters_inventory::fetch_filters(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| FiltersPanelTemplate { panel });
    render_resources_panel(result)
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_secrets.html")]
struct SecretsPanelTemplate {
    panel: data::Panel<secrets_panel::SecretsData>,
}

async fn resources_secrets_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = secrets_panel::fetch_secrets(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| SecretsPanelTemplate { panel });
    render_resources_panel(result)
}

#[derive(askama::Template)]
#[template(path = "dashboard/resources_orphans.html")]
struct OrphansPanelTemplate {
    panel: orphans::OrphansPanel,
}

async fn resources_orphans_partial(
    axum::extract::State(state): axum::extract::State<Arc<DashState>>,
) -> Response {
    let result = orphans::fetch_orphans(&state.client, &state.team, chrono::Utc::now())
        .await
        .map(|panel| OrphansPanelTemplate { panel });
    render_resources_panel(result)
}

async fn htmx_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], HTMX_JS)
}

async fn dashboard_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], DASHBOARD_CSS)
}

async fn resources_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        RESOURCES_JS,
    )
}

#[cfg(test)]
mod container_profile_tests {
    //! fpv2-m4u.1 unit layer: flag parsing, URL derivation, url-file lifecycle helpers.
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use clap::Parser;

    /// Minimal parser wrapper so DashboardOptions parses exactly as a flattened
    /// subcommand does in main.rs.
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        opts: DashboardOptions,
    }

    /// Unique per-test dir under the OS temp dir (std-only, same pattern as the
    /// integration harness `common::unique_tempdir`); removed on drop.
    struct TestDir(PathBuf);
    impl TestDir {
        fn new(tag: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "flowplane-dash-ut-{tag}-{}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn flags_default_to_native_profile() {
        let cli = TestCli::try_parse_from(["dashboard"]).expect("parse");
        assert!(cli.opts.listen.is_none());
        assert!(!cli.opts.no_open);
        assert!(cli.opts.url_file.is_none());
    }

    #[test]
    fn flags_parse_container_profile() {
        let cli = TestCli::try_parse_from([
            "dashboard",
            "--listen",
            "0.0.0.0:8081",
            "--no-open",
            "--url-file",
            "/shared/dashboard-url",
        ])
        .expect("parse");
        let listen = cli.opts.listen.expect("listen");
        assert_eq!(listen, "0.0.0.0:8081".parse::<SocketAddr>().unwrap());
        assert!(!listen.ip().is_loopback());
        assert!(cli.opts.no_open);
        assert_eq!(
            cli.opts.url_file.as_deref(),
            Some(Path::new("/shared/dashboard-url"))
        );
    }

    #[test]
    fn listen_rejects_garbage_address() {
        assert!(TestCli::try_parse_from(["dashboard", "--listen", "not-an-addr"]).is_err());
    }

    #[test]
    fn url_never_derived_from_bind_address() {
        // The derivation function takes only port + nonce — an off-loopback bind cannot
        // leak into the URL because the bind address is not an input at all.
        assert_eq!(
            dashboard_url(8081, "abc123"),
            "http://127.0.0.1:8081/abc123/"
        );
        assert_eq!(
            dashboard_url(65535, "deadbeef"),
            "http://127.0.0.1:65535/deadbeef/"
        );
    }

    #[test]
    fn prepare_url_file_deletes_stale_file() {
        let dir = TestDir::new("t");
        let path = dir.path().join("dashboard-url");
        std::fs::write(&path, "http://127.0.0.1:1/stale-nonce/\n").expect("seed stale");
        prepare_url_file(&path).expect("prepare");
        assert!(
            !path.exists(),
            "stale url-file must be deleted before binding"
        );
    }

    #[test]
    fn prepare_url_file_ok_when_absent() {
        let dir = TestDir::new("t");
        let path = dir.path().join("dashboard-url");
        prepare_url_file(&path).expect("prepare on absent file");
        assert!(!path.exists());
    }

    #[test]
    fn prepare_url_file_fatal_on_unwritable_path() {
        // Parent "directory" is a regular file → the path can never be written.
        let dir = TestDir::new("t");
        let not_a_dir = dir.path().join("blocker");
        std::fs::write(&not_a_dir, b"file").expect("seed");
        let path = not_a_dir.join("dashboard-url");
        let err = prepare_url_file(&path).expect_err("must be fatal");
        assert!(
            format!("{err:#}").contains("not writable")
                || format!("{err:#}").contains("delete stale"),
            "error must name the url-file problem: {err:#}"
        );
    }

    #[test]
    fn write_url_file_atomic_leaves_only_the_target() {
        let dir = TestDir::new("t");
        let path = dir.path().join("dashboard-url");
        write_url_file_atomic(&path, "http://127.0.0.1:8081/abc/").expect("write");
        let content = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(content, "http://127.0.0.1:8081/abc/\n");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|e| e.expect("entry").file_name())
            .filter(|n| n != "dashboard-url")
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files may remain: {leftovers:?}"
        );
    }

    #[test]
    fn sibling_path_stays_in_same_directory() {
        let p = sibling_path(Path::new("/shared/dashboard-url"), "tmp");
        assert_eq!(p.parent(), Some(Path::new("/shared")));
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dashboard-url.tmp-"));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::cli::config::EffectiveConfig;

    fn test_state(nonce: &str, port: u16) -> Arc<DashState> {
        Arc::new(DashState {
            nonce: nonce.to_string(),
            port,
            client: RestClient::for_tests(EffectiveConfig {
                server: "http://127.0.0.1:9".to_string(),
                org: Some("test-org".into()),
                team: Some("test-team".into()),
                token: Some("test-bearer-token".into()),
                token_source: None,
                dev_fallback_available: false,
                timeout: 5,
                oidc_issuer: None,
                oidc_client_id: None,
                oidc_scope: None,
                callback_url: None,
            }),
            team: "test-team".into(),
        })
    }

    /// Serve a router with a KNOWN nonce on an ephemeral loopback port (parallel-safe).
    async fn spawn(nonce: &str) -> (String, u16) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let app = build_router(test_state(nonce, port));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://127.0.0.1:{port}"), port)
    }

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn nonce_is_128_bit_hex_and_unique_per_launch() {
        let a = generate_nonce().expect("nonce");
        let b = generate_nonce().expect("nonce");
        assert_eq!(a.len(), 32, "16 bytes hex-encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two launches must not share a nonce");
    }

    #[tokio::test]
    async fn every_route_class_requires_the_nonce() {
        let (base, _) = spawn(NONCE).await;
        let http = reqwest::Client::new();
        for path in ROUTE_PATHS {
            // Without any nonce prefix → 404.
            let bare = http
                .get(format!("{base}{path}"))
                .send()
                .await
                .expect("send");
            assert_eq!(
                bare.status(),
                reqwest::StatusCode::NOT_FOUND,
                "unnonced {path} must 404"
            );
            // With a wrong nonce → 404.
            let wrong = http
                .get(format!("{base}/deadbeefdeadbeefdeadbeefdeadbeef{path}"))
                .send()
                .await
                .expect("send");
            assert_eq!(
                wrong.status(),
                reqwest::StatusCode::NOT_FOUND,
                "wrong-nonce {path} must 404"
            );
            // With the launch nonce → 200.
            let ok = http
                .get(format!("{base}/{NONCE}{path}"))
                .send()
                .await
                .expect("send");
            assert_eq!(
                ok.status(),
                reqwest::StatusCode::OK,
                "nonced {path} must serve"
            );
        }
    }

    #[tokio::test]
    async fn non_get_methods_are_rejected() {
        let (base, _) = spawn(NONCE).await;
        let http = reqwest::Client::new();
        for method in [
            reqwest::Method::POST,
            reqwest::Method::PUT,
            reqwest::Method::DELETE,
            reqwest::Method::PATCH,
        ] {
            let res = http
                .request(method.clone(), format!("{base}/{NONCE}/"))
                .send()
                .await
                .expect("send");
            assert_eq!(
                res.status(),
                reqwest::StatusCode::METHOD_NOT_ALLOWED,
                "{method} on a dashboard route must 405"
            );
        }
    }

    #[tokio::test]
    async fn security_headers_on_every_response_including_errors() {
        let (base, _) = spawn(NONCE).await;
        let http = reqwest::Client::new();
        for url in [format!("{base}/{NONCE}/"), format!("{base}/nope")] {
            let res = http.get(&url).send().await.expect("send");
            let h = res.headers();
            assert_eq!(h.get("cache-control").unwrap(), "no-store", "{url}");
            assert_eq!(
                h.get("content-security-policy").unwrap(),
                "default-src 'self'",
                "{url}"
            );
            assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer", "{url}");
            assert_eq!(h.get("x-frame-options").unwrap(), "DENY", "{url}");
        }
    }

    #[tokio::test]
    async fn head_is_rejected_like_any_non_get() {
        // axum's `get()` would serve HEAD too; the design says GET only (AC 3).
        let (base, _) = spawn(NONCE).await;
        let res = reqwest::Client::new()
            .head(format!("{base}/{NONCE}/"))
            .send()
            .await
            .expect("send");
        assert_eq!(res.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn missing_nonce_is_404_even_with_foreign_host_or_origin() {
        // Contract precedence (AC 2): an unknown/no-nonce path is 404 regardless of the
        // request's Host/Origin; 403 is reserved for real routes with foreign headers.
        let (base, _) = spawn(NONCE).await;
        let http = reqwest::Client::new();
        let with_host = http
            .get(format!("{base}/"))
            .header("Host", "evil.example")
            .send()
            .await
            .expect("send");
        assert_eq!(with_host.status(), reqwest::StatusCode::NOT_FOUND);
        let with_origin = http
            .get(format!("{base}/assets/htmx.min.js"))
            .header("Origin", "http://evil.example")
            .send()
            .await
            .expect("send");
        assert_eq!(with_origin.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn foreign_host_is_rejected() {
        let (base, _) = spawn(NONCE).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/{NONCE}/"))
            .header("Host", "evil.example")
            .send()
            .await
            .expect("send");
        assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn foreign_origin_is_rejected_own_origin_allowed() {
        let (base, port) = spawn(NONCE).await;
        let http = reqwest::Client::new();
        let evil = http
            .get(format!("{base}/{NONCE}/"))
            .header("Origin", "http://evil.example")
            .send()
            .await
            .expect("send");
        assert_eq!(evil.status(), reqwest::StatusCode::FORBIDDEN);
        let own = http
            .get(format!("{base}/{NONCE}/"))
            .header("Origin", format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .expect("send");
        assert_eq!(own.status(), reqwest::StatusCode::OK);
    }

    #[test]
    fn host_allowlist_logic() {
        assert!(host_allowed("127.0.0.1:9000", 9000));
        assert!(host_allowed("localhost:9000", 9000));
        assert!(host_allowed("127.0.0.1", 9000));
        assert!(!host_allowed("127.0.0.1:9001", 9000), "port must match");
        assert!(!host_allowed("evil.example:9000", 9000));
        assert!(!host_allowed("127.0.0.1.evil.example:9000", 9000));
        assert!(!host_allowed("", 9000));
    }

    #[test]
    fn team_segment_charset_blocks_path_escape() {
        assert!(validate_team_segment("payments").is_ok());
        assert!(validate_team_segment("team-a2").is_ok());
        assert!(validate_team_segment("0198f2f3-1111-7000-8000-000000000000").is_ok());
        for hostile in [
            "",
            "a/b",
            "..",
            "a?x=1",
            "a#frag",
            "a%2Fb",
            "a b",
            "../admin",
            &"x".repeat(101),
        ] {
            assert!(
                validate_team_segment(hostile).is_err(),
                "must reject {hostile:?}"
            );
        }
    }

    #[test]
    fn domain_query_decoding_round_trips_the_encoder() {
        for raw in [
            "checkout",
            "a|b",
            "a/b c%",
            "dom?x=1#f",
            "multi|part|domain",
        ] {
            let encoded = resources::encode_segment(raw);
            let query = format!("domain={encoded}");
            assert_eq!(
                domain_from_query(Some(&query)).as_deref(),
                Some(raw),
                "decode(encode({raw:?})) must round-trip"
            );
        }
        assert_eq!(domain_from_query(None), None);
        assert_eq!(domain_from_query(Some("other=1")), None);
        assert_eq!(
            domain_from_query(Some("domain=%zz")),
            None,
            "malformed percent escapes are rejected"
        );
        // A 253-CHAR (but >253-byte) Unicode domain is legal per the CP contract and
        // must survive the round-trip; the handler validates chars, not bytes.
        let unicode = "é".repeat(253);
        assert!(fp_domain::rate_limit::validate_rate_limit_domain_name(&unicode).is_ok());
        let encoded = resources::encode_segment(&unicode);
        assert_eq!(
            domain_from_query(Some(&format!("domain={encoded}"))).as_deref(),
            Some(unicode.as_str())
        );
    }

    #[test]
    fn listen_flag_owned_by_container_profile() {
        use clap::Parser as _;
        // F1 shipped no `--listen` at all; the approved F3 design (ui-f3-eval-funnel,
        // fpv2-m4u.1) transfers ownership: the flag now parses, the default profile
        // stays loopback-ephemeral, and the URL is never derived from the bind address
        // (covered in container_profile_tests).
        assert!(
            crate::Cli::try_parse_from(["flowplane", "dashboard", "--listen", "0.0.0.0:80"])
                .is_ok()
        );
        assert!(crate::Cli::try_parse_from(["flowplane", "dashboard"]).is_ok());
    }

    #[tokio::test]
    async fn overview_shell_renders_without_token_material() {
        let (base, _) = spawn(NONCE).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/{NONCE}/"))
            .send()
            .await
            .expect("send");
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let body = res.text().await.expect("body");
        assert!(body.contains("Flowplane"), "shell renders");
        assert!(
            !body.contains("test-bearer-token"),
            "bearer token must never reach HTML"
        );
    }
}

//! bridge.rs: a loopback HTTP surface that lets an EXTERNAL host run the very same HUD
//! bundle this app ships, in its own web view.
//!
//! WHY THIS EXISTS. The boring.notch fork carries a WARDEN section, and the one property
//! that section has to have is that its globes are the SAME globes. Not a Swift
//! reimplementation that tracks ours: a `RadarGlobeBody` is a transmission
//! `meshPhysicalMaterial` gem inside a wireframe shell with a point-cloud lattice and an
//! emissive core well past 1.0, resolved by a bloom pass against an environment probe.
//! Restating that in SceneKit would be a second renderer of one object, and the
//! 2026-08-20 decision already recorded what that costs: the HUD used to own a cheaper
//! globe and the two "drifted on sight, which is what makes two screens read as two
//! products". A port re-opens that split across a LANGUAGE boundary, where nothing can be
//! shared and nothing can be tested against the other. So the section runs our bundle.
//!
//! WHAT THE SEAM IS. `HudRoot` touches Tauri in exactly two ways: `invoke` for commands
//! and `listen` for events. Everything below that line is pure React and Three. So this
//! module restates those two verbs over loopback HTTP, and the frontend picks a transport
//! at runtime. No view code, no scene code and no globe code knows which host it is in.
//!
//! WHAT THIS IS NOT. It is not a remote surface and it is not `observe/`. `observe/`
//! shares REDACTED frames with another person over iroh and its redaction is the whole
//! point of it. This serves UN-redacted local state to a process running as the same user
//! on the same machine, which is why it never leaves loopback and why it is
//! token-gated. The two must not be confused and must not share a path.
//!
//! SECURITY POSTURE, all of it enforced here rather than assumed:
//!   - Bound to 127.0.0.1 ONLY. Never 0.0.0.0, so nothing off this machine can reach it.
//!   - Every request carries a token minted fresh at each launch, compared in constant
//!     time. The token is written to `~/.warden/bridge.json` at mode 0600, which is the
//!     only place the section reads it from: a process that cannot read that file cannot
//!     drive WARDEN.
//!   - `Origin` is checked and a foreign one is refused. Loopback is NOT a trust
//!     boundary for a browser: any web page the operator visits can POST to 127.0.0.1,
//!     so the token alone would leave `hud_focus_agent` reachable from a hostile tab.
//!   - The command surface is an ALLOW-LIST of the five the HUD actually calls, matched
//!     by name. A new Tauri command does not become reachable here by existing.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::broadcast;

/// One `listen`-able event, in the shape the frontend transport re-emits it.
#[derive(Clone, Debug, Serialize)]
pub struct BridgeEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

struct Bridge {
    token: String,
    tx: broadcast::Sender<BridgeEvent>,
}

static BRIDGE: OnceCell<Bridge> = OnceCell::new();

/// Push an event to every attached external host.
///
/// Deliberately infallible and deliberately cheap: this sits on the radar recompute path
/// next to the `app.emit` it mirrors, and a bridge nobody has attached to must cost that
/// path nothing. `broadcast::Sender::send` returning `Err` only means no receivers, which
/// is the normal state when the section is closed.
pub fn publish(event: &str, payload: serde_json::Value) {
    if let Some(b) = BRIDGE.get() {
        let _ = b.tx.send(BridgeEvent {
            event: event.to_string(),
            payload,
        });
    }
}

/// Where the handshake file lives. The section reads port and token from here.
fn handshake_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".warden").join("bridge.json"))
}

/// Write `{port, token}` where only this user can read it.
///
/// Created at 0600 BEFORE the token is written, not chmod'ed after: a file that is
/// world-readable for even the width of one syscall is a file that leaked.
fn write_handshake(port: u16, token: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let Some(path) = handshake_path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Truncate through the same handle so the mode applies to the file we then write.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    let body = serde_json::json!({ "port": port, "token": token });
    f.write_all(body.to_string().as_bytes())?;
    Ok(())
}

/// Remove the handshake file.
///
/// A stale `bridge.json` names a port nothing is listening on and a token nothing will
/// accept, which turns a clean "WARDEN is not running" into a section that hangs on a
/// refused connection. Called on shutdown.
pub fn clear_handshake() {
    if let Some(p) = handshake_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// A 256-bit token, hex encoded.
fn mint_token() -> String {
    use rand::Rng;
    let mut raw = [0u8; 32];
    // The same mint `observe::grants` uses for its share secret: `rand::rng()` is a
    // CSPRNG seeded from the OS, and matching the one already trusted here beats
    // introducing a second opinion about where this app gets its secrets.
    rand::rng().fill_bytes(&mut raw);
    hex::encode(raw)
}

/// Constant-time token check.
///
/// `==` on a `String` returns on the first differing byte, which leaks the length of the
/// matching prefix and makes the token guessable one byte at a time.
fn token_ok(supplied: &str) -> bool {
    let Some(b) = BRIDGE.get() else {
        return false;
    };
    supplied.as_bytes().ct_eq(b.token.as_bytes()).into()
}

/// Reject an `Origin` that is not our own.
///
/// Absent is allowed: a native `WKWebView` navigating to our page sends no `Origin` on a
/// plain GET, and neither does a same-origin fetch in some WebKit versions. Present and
/// foreign is refused, which is the case that matters, because a page on the open web
/// CAN reach 127.0.0.1 and would otherwise be one stolen token away from driving WARDEN.
fn origin_ok(headers: &HeaderMap, port: u16) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Pull the token from `Authorization: Bearer`, falling back to `?token=`.
///
/// The query fallback exists for exactly one caller: `EventSource` cannot set headers, so
/// an SSE stream has no other way to authenticate. Every other route uses the header.
fn supplied_token(headers: &HeaderMap, q: &TokenQuery) -> String {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| q.token.clone())
        .unwrap_or_default()
}

#[derive(Clone)]
struct Ctx {
    app: tauri::AppHandle,
    port: u16,
}

#[derive(Deserialize)]
struct InvokeBody {
    cmd: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// The HUD's command surface, restated for an external host.
///
/// An ALLOW-LIST, not a dispatch into every registered command: `commands.rs` holds
/// plenty that an embedded panel has no business calling, and a `match` on a name is the
/// only form of this that does not grow a hole every time a command is added.
///
/// Three of the five are deliberately no-ops here. `hud_hide`, `hud_take_focus` and
/// `hud_pending_summon` all drive OUR panel's window lifecycle, and in the section there
/// is no such window: boring.notch owns the panel, its chrome and its dismissal. Making
/// them errors instead would be worse, because `HudRoot` calls them on paths it does not
/// branch on, and every one would surface as a caught-and-swallowed rejection.
async fn dispatch(app: &tauri::AppHandle, cmd: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    match cmd {
        "get_radar_state" => {
            use tauri::Manager;
            let state = app.state::<crate::commands::AppState>();
            let radar = crate::commands::fresh_radar_state_for_read(&state);
            serde_json::to_value(radar).map_err(|e| e.to_string())
        }
        "hud_focus_agent" => {
            // camelCase on the wire because that is what `invoke` sends and the frontend
            // call site is unchanged; snake_case accepted too so a hand-made request works.
            let agent_id = args
                .get("agentId")
                .or_else(|| args.get("agent_id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "hud_focus_agent: agentId required".to_string())?;
            crate::commands::hud_focus_agent(app.clone(), agent_id.to_string()).await?;
            Ok(serde_json::Value::Null)
        }
        "diag" => {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            crate::commands::diag(format!("[section] {msg}"));
            Ok(serde_json::Value::Null)
        }
        // Window lifecycle: the host owns it here. See the note above.
        "hud_hide" | "hud_take_focus" => Ok(serde_json::Value::Null),
        // ALWAYS summoned, and this must not become `null`. `HudRoot` reads a null
        // pending-summon as "this window is up with no panel on it", concludes it is
        // stranded and hides itself, which in the section is a permanently blank tab. In
        // our own window that deduction is right; here the host decides when the panel is
        // on screen, so being asked at all means it is. An empty object takes every
        // default in `applySummon`: centred, 24px neck, and `auto: false`, which is the
        // one that matters, because an auto summon leaves again after 6s and a section
        // must not walk out of its own tab.
        "hud_pending_summon" => Ok(serde_json::json!({})),
        other => Err(format!("command not exposed on the bridge: {other}")),
    }
}

async fn invoke_route(
    State(ctx): State<Ctx>,
    Query(q): Query<TokenQuery>,
    headers: HeaderMap,
    Json(body): Json<InvokeBody>,
) -> impl IntoResponse {
    if !origin_ok(&headers, ctx.port) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"origin"}))).into_response();
    }
    if !token_ok(&supplied_token(&headers, &q)) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"token"}))).into_response();
    }
    match dispatch(&ctx.app, &body.cmd, &body.args).await {
        Ok(v) => (StatusCode::OK, Json(serde_json::json!({ "ok": v }))).into_response(),
        Err(e) => (StatusCode::OK, Json(serde_json::json!({ "err": e }))).into_response(),
    }
}

/// The `listen` half: one SSE stream carrying every published event.
///
/// SSE rather than a WebSocket on purpose. The only traffic that has to be pushed is
/// `radar_state` and `hud_dismiss`, both server to client, and SSE costs no new
/// dependency, no framing code and no masking. `EventSource` also reconnects on its own,
/// so a WARDEN restart heals the section without the section knowing it happened.
async fn events_route(
    State(ctx): State<Ctx>,
    Query(q): Query<TokenQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !origin_ok(&headers, ctx.port) {
        return (StatusCode::FORBIDDEN, "origin").into_response();
    }
    if !token_ok(&supplied_token(&headers, &q)) {
        return (StatusCode::UNAUTHORIZED, "token").into_response();
    }
    let Some(b) = BRIDGE.get() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no bridge").into_response();
    };
    let mut rx = b.tx.subscribe();

    // The first frame is the CURRENT state, not the next recompute. A section that
    // attached between two recomputes would otherwise paint an empty forest and hold it
    // for a whole tick, which reads as "no agents" rather than "not asked yet".
    let app = ctx.app.clone();
    let stream = async_stream::stream! {
        if let Ok(v) = dispatch(&app, "get_radar_state", &serde_json::Value::Null).await {
            if let Ok(ev) = SseEvent::default().json_data(BridgeEvent {
                event: "radar_state".into(),
                payload: v,
            }) {
                yield Ok::<_, std::convert::Infallible>(ev);
            }
        }
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if let Ok(ev) = SseEvent::default().json_data(msg) {
                        yield Ok(ev);
                    }
                }
                // Lagged means this receiver fell behind the ring buffer. Keep the stream
                // alive: the next `radar_state` is a FULL state, so a dropped frame costs
                // one tick of staleness, never a corrupt view.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

/// Liveness plus the one number the host needs to build its URLs. Unauthenticated on
/// purpose: it answers "is WARDEN up" and nothing else, and the section needs that answer
/// before it has read a token.
async fn health_route() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "app": "warden" }))
}

/// Locate the built frontend.
///
/// In a bundled `.app` the assets ship as a resource directory; in `tauri dev` they are
/// still in the repo's `dist/`. Checked in that order so a stale repo `dist/` can never
/// shadow what an installed build actually ships.
fn dist_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    if let Ok(res) = app.path().resource_dir() {
        let candidate = res.join("dist");
        if candidate.join("hud.html").exists() {
            return Some(candidate);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist");
    if dev.join("hud.html").exists() {
        return Some(dev);
    }
    None
}

/// Start the bridge. Idempotent: a second call is a no-op.
///
/// Binds port 0 and lets the OS choose. A fixed port would collide with whatever else the
/// operator runs and, worse, would make the surface guessable; the handshake file is how
/// the section learns the number, and it needs the token from that file anyway.
pub fn spawn(app: tauri::AppHandle) {
    if BRIDGE.get().is_some() {
        return;
    }
    let token = mint_token();
    let (tx, _) = broadcast::channel::<BridgeEvent>(64);

    let listener = match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "bridge: could not bind loopback; section disabled");
            return;
        }
    };
    let port = match listener.local_addr() {
        Ok(SocketAddr::V4(a)) => a.port(),
        Ok(SocketAddr::V6(a)) => a.port(),
        Err(e) => {
            tracing::warn!(error = %e, "bridge: no local addr; section disabled");
            return;
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        tracing::warn!(error = %e, "bridge: set_nonblocking failed; section disabled");
        return;
    }

    if BRIDGE
        .set(Bridge {
            token: token.clone(),
            tx,
        })
        .is_err()
    {
        return;
    }
    if let Err(e) = write_handshake(port, &token) {
        tracing::warn!(error = %e, "bridge: handshake not written; section cannot attach");
    }

    let ctx = Ctx {
        app: app.clone(),
        port,
    };
    let mut router = Router::new()
        .route("/health", get(health_route))
        .route("/events", get(events_route))
        .route("/invoke", post(invoke_route));

    // Static assets last, so a route above always wins over a file that happens to share
    // its name. `hud.html` is the index: the section navigates to `/` and gets the panel.
    if let Some(dist) = dist_dir(&app) {
        router = router.fallback_service(
            tower_http::services::ServeDir::new(&dist)
                .append_index_html_on_directories(false)
                .fallback(tower_http::services::ServeFile::new(dist.join("hud.html"))),
        );
    } else {
        tracing::warn!("bridge: no dist/ found; section will serve API only");
    }

    let router = router.with_state(ctx);

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "bridge: listener adoption failed");
                return;
            }
        };
        tracing::info!(port, "bridge listening on loopback");
        if let Err(e) = axum::serve(listener, router).await {
            tracing::warn!(error = %e, "bridge: serve ended");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_absent_is_allowed() {
        // A WKWebView navigating to the page sends no Origin; refusing that would refuse
        // the only caller this bridge has.
        assert!(origin_ok(&HeaderMap::new(), 5000));
    }

    #[test]
    fn foreign_origin_is_refused() {
        // The case that matters: a page on the open web CAN reach loopback.
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!origin_ok(&h, 5000));
    }

    #[test]
    fn own_origin_is_allowed() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::ORIGIN, "http://127.0.0.1:5000".parse().unwrap());
        assert!(origin_ok(&h, 5000));
        let mut h2 = HeaderMap::new();
        h2.insert(axum::http::header::ORIGIN, "http://127.0.0.1:5001".parse().unwrap());
        assert!(!origin_ok(&h2, 5000));
    }

    #[test]
    fn header_token_beats_query() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        let q = TokenQuery {
            token: Some("xyz".into()),
        };
        assert_eq!(supplied_token(&h, &q), "abc");
    }

    #[test]
    fn query_token_is_the_sse_fallback() {
        let q = TokenQuery {
            token: Some("xyz".into()),
        };
        assert_eq!(supplied_token(&HeaderMap::new(), &q), "xyz");
    }

    #[test]
    fn missing_token_is_empty_not_a_panic() {
        let q = TokenQuery { token: None };
        assert_eq!(supplied_token(&HeaderMap::new(), &q), "");
    }

    #[test]
    fn minted_tokens_are_256_bit_and_distinct() {
        let a = mint_token();
        let b = mint_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}

//! HGhub 网页版服务端。
//!
//! 设计要点（对应实测结论）：
//! 1. 不重写业务：数据抓取、封面 AES 解密、HLS 清单改写/分片转发全部复用桌面端 Rust 核心
//!    （net.rs / media.rs / lines.rs / session.rs / network_policy.rs / logging.rs / playback_log.rs）。
//! 2. 前端只多一层 `invoke → fetch`：命令原名不变，参数与返回结构不变。
//! 3. 媒体走 HTTP 代理端点 `/hls` `/seg` `/img`，替掉桌面端的 `hg://` 自定义协议。
//! 4. 全站口令登录门；静态资源也在门内。
#![allow(dead_code)]

mod auth;
mod lines;
mod logging;
mod media;
mod net;
mod network_policy;
mod playback_log;
mod rpc;
mod session;

use axum::{
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct Api {
    pub session: session::SessionStore,
}

pub struct AppState {
    pub api: Api,
    pub data_dir: PathBuf,
    pub dist: PathBuf,
    pub password: String,
    pub auth: Arc<auth::Auth>,
    pub login_hits: Mutex<HashMap<String, Vec<Instant>>>,
}

type St = Arc<AppState>;

#[tokio::main]
async fn main() {
    let bind = std::env::var("HGHUB_BIND").unwrap_or_else(|_| "127.0.0.1:8900".into());
    let password = std::env::var("HGHUB_PASSWORD").unwrap_or_default();
    if password.trim().len() < 6 {
        eprintln!("[fatal] 环境变量 HGHUB_PASSWORD 未设置或短于 6 位，拒绝以无口令状态启动");
        std::process::exit(2);
    }
    let data_dir = PathBuf::from(std::env::var("HGHUB_DATA").unwrap_or_else(|_| "./data".into()));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let dist = PathBuf::from(std::env::var("HGHUB_DIST").unwrap_or_else(|_| "./dist".into()));
    if !dist.join("index.html").exists() {
        eprintln!("[fatal] 前端产物缺失：{} 下没有 index.html", dist.display());
        std::process::exit(3);
    }

    // 复用桌面端的落盘逻辑：线路、安全模式、会话、播放日志
    net::bind_base(&data_dir);
    network_policy::bind_security(&data_dir);
    playback_log::bind(&data_dir);

    // 口令盐：首启生成并落盘，改动会让所有已登录会话失效
    let salt_path = data_dir.join("auth-salt.bin");
    let salt = if let Ok(b) = std::fs::read(&salt_path) {
        b
    } else {
        let mut s = vec![0u8; 32];
        {
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut s);
        }
        let _ = std::fs::write(&salt_path, &s);
        s
    };

    let state = Arc::new(AppState {
        api: Api { session: session::SessionStore::new() },
        data_dir: data_dir.clone(),
        dist: dist.clone(),
        password: password.clone(),
        auth: Arc::new(auth::Auth::new(&password, &salt)),
        login_hits: Mutex::new(HashMap::new()),
    });
    state.api.session.bind(data_dir.clone());



    let app = Router::new()
        .route("/login", get(login_page).post(login_submit))
        .route("/logout", post(logout))
        .route("/rpc/{cmd}", post(rpc_handler))
        .route("/hls", get(media_route))
        .route("/seg", get(media_route))
        .route("/img", get(media_route))
        .route("/media/hls", get(media_route))
        .route("/media/seg", get(media_route))
        .route("/media/img", get(media_route))
        .route("/healthz", get(healthz))
        .fallback(get(static_file))
        .layer(middleware::from_fn_with_state(state.clone(), auth::gate))
        .with_state(state.clone());

    let addr: SocketAddr = bind.parse().expect("HGHUB_BIND 形如 127.0.0.1:8900");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind failed");
    eprintln!(
        "[boot] HGhub 网页版已监听 http://{}  dist={} data={}",
        addr,
        dist.display(),
        data_dir.display()
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("[boot] 收到退出信号");
        })
        .await
        .expect("server error");
}

async fn healthz() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

struct Json(Value);
impl IntoResponse for Json {
    fn into_response(self) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(self.0.to_string()))
            .unwrap()
    }
}

async fn login_page(Query(q): Query<HashMap<String, String>>) -> Response {
    auth::login_page(q.get("next").map(String::as_str).unwrap_or(""))
}

fn client_ip(h: &HeaderMap) -> String {
    h.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "local".into())
}

async fn login_submit(
    State(st): State<St>,
    headers: HeaderMap,
    body: String,
) -> Response {
    // 简单限速：同一 IP 每分钟最多 6 次尝试
    let ip = client_ip(&headers);
    {
        let mut hits = st.login_hits.lock().unwrap();
        let now = Instant::now();
        let e = hits.entry(ip.clone()).or_default();
        e.retain(|t| now.duration_since(*t).as_secs() < 60);
        if e.len() >= 6 {
            return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"ok": false, "error": "too_many"}))).into_response();
        }
        e.push(now);
    }
    let given = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("password").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default();
    if !st.auth.check_password(&given, &st.password) {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false}))).into_response();
    }
    let token = st.auth.issue();
    // 反向代理终止 TLS 时，cookie 由代理侧决定是否加 Secure；这里按请求协议判断
    let secure = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false);
    let mut resp = Json(json!({"ok": true})).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        auth::set_cookie(&token, secure).parse().unwrap(),
    );
    resp
}

async fn logout() -> Response {
    let mut resp = Json(json!({"ok": true})).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        auth::clear_cookie().parse().unwrap(),
    );
    resp
}

async fn rpc_handler(
    State(st): State<St>,
    AxPath(cmd): AxPath<String>,
    body: String,
) -> Response {
    let args: Value = if body.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": format!("bad json: {e}")}))).into_response()
            }
        }
    };
    // Tauri 命令名可能带 plugin: 前缀，网页版统一当无操作
    if cmd.starts_with("plugin:") {
        return Json(json!({"ok": true, "data": null})).into_response();
    }
    match rpc::dispatch(st, &cmd, args).await {
        Ok(data) => Json(json!({"ok": true, "data": data})).into_response(),
        Err(err) => Json(json!({"ok": false, "error": err})).into_response(),
    }
}

/// /hls /seg /img：把桌面端的自定义协议处理器原样搬到 HTTP 上。
/// origin 传空串，改写出来的地址就是同源相对路径（/seg?u=…），浏览器直接走代理。
async fn media_route(State(_st): State<St>, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches("/media").to_string();
    let query = uri.query().unwrap_or("").to_string();
    media::handle(&path, &query, "").await.map(Body::from)
}

async fn static_file(State(st): State<St>, uri: axum::http::Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let rel = if raw.is_empty() { "index.html" } else { raw };
    let mut target = st.dist.clone();
    for seg in rel.split('/') {
        if seg == ".." || seg.contains('\\') {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        target.push(seg);
    }
    let root = st.dist.canonicalize().unwrap_or(st.dist.clone());
    if let Ok(resolved) = target.canonicalize() {
        if !resolved.starts_with(&root) {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        if resolved.is_file() {
            if let Ok(bytes) = std::fs::read(&resolved) {
                let ctype = mime_of(&resolved);
                let cache = if rel.starts_with("assets/") {
                    "public, max-age=31536000, immutable"
                } else {
                    "no-cache"
                };
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, ctype)
                    .header(header::CACHE_CONTROL, cache)
                    .body(Body::from(bytes))
                    .unwrap();
            }
        }
    }
    // SPA 路由回退到 index.html
    match std::fs::read(st.dist.join("index.html")) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => (StatusCode::NOT_FOUND, Html("not found")).into_response(),
    }
}

fn mime_of(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

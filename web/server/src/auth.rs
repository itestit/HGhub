//! 网页版登录门：单一口令 + HMAC 签名 Cookie。
//! 未登录时任何路径（含静态资源）都回 302 到 /login，登录页自身内联样式，不依赖任何外部资源。
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

pub const COOKIE: &str = "hgweb";
const TTL_SECS: u64 = 60 * 60 * 24 * 30;

#[derive(Clone)]
pub struct Auth {
    key: Vec<u8>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Auth {
    /// 口令只从环境变量读；密钥 = SHA256(口令 + 落盘的随机盐)，盐丢失/轮换会让所有会话失效。
    pub fn new(password: &str, salt: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(password.as_bytes());
        h.update(salt);
        Self { key: h.finalize().to_vec() }
    }

    fn mac(&self, payload: &str) -> String {
        let mut m = HmacSha256::new_from_slice(&self.key).expect("hmac key");
        m.update(payload.as_bytes());
        m.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn issue(&self) -> String {
        let exp = now() + TTL_SECS;
        let nonce: String = {
            use rand::RngCore;
            let mut b = [0u8; 8];
            rand::thread_rng().fill_bytes(&mut b);
            b.iter().map(|x| format!("{x:02x}")).collect()
        };
        let payload = format!("v1.{exp}.{nonce}");
        format!("{payload}.{}", self.mac(&payload))
    }

    pub fn verify(&self, token: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 4 || parts[0] != "v1" {
            return false;
        }
        let payload = format!("v1.{}.{}", parts[1], parts[2]);
        if !ct_eq(&self.mac(&payload), parts[3]) {
            return false;
        }
        parts[1].parse::<u64>().map(|exp| exp > now()).unwrap_or(false)
    }

    pub fn check_password(&self, given: &str, expected: &str) -> bool {
        // 长度与时序都尽量常数化，避免口令枚举
        let a = Sha256::digest(given.as_bytes());
        let b = Sha256::digest(expected.as_bytes());
        ct_eq(&hex(&a), &hex(&b))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn cookie_from(headers: &http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let mut it = part.trim().splitn(2, '=');
        if it.next() == Some(COOKIE) {
            return it.next().map(|s| s.to_string());
        }
    }
    None
}

pub fn set_cookie(token: &str, secure: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={TTL_SECS}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

pub fn is_public(path: &str) -> bool {
    matches!(path, "/login" | "/login.html" | "/favicon.ico")
}

pub async fn gate(State(st): State<Arc<crate::AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if is_public(&path) {
        return next.run(req).await;
    }
    let ok = cookie_from(req.headers())
        .map(|t| st.auth.verify(&t))
        .unwrap_or(false);
    if ok {
        return next.run(req).await;
    }
    // 未登录：页面请求跳登录页；RPC/媒体请求回 401，让前端能区分
    if path.starts_with("/rpc/")
        || path.starts_with("/hls")
        || path.starts_with("/seg")
        || path.starts_with("/img")
        || path.starts_with("/media/")
    {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    Redirect::to("/login").into_response()
}

pub fn login_page(next: &str) -> Response {
    let next = next.replace('"', "");
    let html = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">
<title>HGhub · 登录</title>
<style>
:root{{color-scheme:dark}}
*{{box-sizing:border-box}}
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;
background:#0b0b0d;color:#e8e8ea;font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",sans-serif;
padding:max(24px,env(safe-area-inset-top)) 24px}}
.card{{width:100%;max-width:340px}}
h1{{font-size:20px;margin:0 0 4px;font-weight:600;letter-spacing:.02em}}
p.sub{{margin:0 0 28px;color:#8b8b93;font-size:13px}}
input{{width:100%;padding:14px 16px;border-radius:12px;border:1px solid #2a2a31;background:#15151a;color:#e8e8ea;
font-size:16px;outline:none}}
input:focus{{border-color:#5b5bd6}}
button{{width:100%;margin-top:14px;padding:14px;border:0;border-radius:12px;background:#5b5bd6;color:#fff;
font-size:16px;font-weight:600;cursor:pointer}}
button:disabled{{opacity:.55}}
.err{{color:#ff6b6b;font-size:13px;min-height:18px;margin-top:10px}}
.foot{{margin-top:26px;color:#5c5c66;font-size:12px;text-align:center}}
</style></head><body><div class="card">
<h1>HGhub</h1><p class="sub">输入访问口令继续</p>
<form id="f"><input id="pw" type="password" autocomplete="current-password" placeholder="访问口令" autofocus>
<button id="b" type="submit">进入</button></form>
<div class="err" id="e"></div>
<div class="foot">私人部署 · 仅本人使用</div>
</div><script>
const f=document.getElementById('f'),pw=document.getElementById('pw'),b=document.getElementById('b'),e=document.getElementById('e');
f.addEventListener('submit',async ev=>{{
  ev.preventDefault();e.textContent='';b.disabled=true;b.textContent='验证中…';
  try{{
    const r=await fetch('/login',{{method:'POST',headers:{{'content-type':'application/json'}},
      body:JSON.stringify({{password:pw.value}})}});
    const j=await r.json().catch(()=>({{}}));
    if(r.ok&&j.ok){{location.replace({next:?});return}}
    e.textContent=r.status===429?'尝试过于频繁，请稍后':'口令不对';
  }}catch(_){{e.textContent='网络错误，请重试'}}
  b.disabled=false;b.textContent='进入';
}});
</script></body></html>"#,
        next = if next.is_empty() || next == "/login" {
            "\"/\"".to_string()
        } else {
            format!("\"{}\"", next)
        }
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(html))
        .unwrap()
}

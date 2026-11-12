//! 把桌面端 `#[tauri::command]` 的命令面对等地暴露成 HTTP RPC。
//! 业务实现全部落在复用的核心模块里（net / media / lines / session），本文件只做参数搬运。
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;

fn s(v: Option<&Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or_default().to_string()
}
fn u(v: Option<&Value>, d: u32) -> u32 {
    v.and_then(|x| x.as_u64()).map(|x| x as u32).unwrap_or(d)
}
fn i(v: Option<&Value>) -> i64 {
    v.and_then(|x| x.as_i64())
        .or_else(|| v.and_then(|x| x.as_str()).and_then(|x| x.parse().ok()))
        .unwrap_or(0)
}
fn b(v: Option<&Value>) -> bool {
    v.and_then(|x| x.as_bool()).unwrap_or(false)
}
fn get<'a>(a: &'a Value, key: &str) -> Option<&'a Value> {
    a.get(key)
}
/// 同时接受 snake_case 与 Tauri 的 camelCase 参数名
fn get2<'a>(a: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    a.get(snake).or_else(|| a.get(camel))
}

async fn api_get(st: &Arc<AppState>, path: &str) -> Result<Value, String> {
    let origin = crate::net::base();
    let token = st.api.session.token(&origin);
    crate::net::edge(&origin, "GET", path, None, token.as_deref()).await
}

async fn api_post(st: &Arc<AppState>, path: &str, body: Option<Value>) -> Result<Value, String> {
    let origin = crate::net::base();
    let token = st.api.session.token(&origin);
    crate::net::edge(&origin, "POST", path, body, token.as_deref()).await
}

async fn api_authed(
    st: &Arc<AppState>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let origin = crate::net::base();
    let token = st
        .api
        .session
        .token(&origin)
        .ok_or_else(|| "请先登录".to_string())?;
    crate::net::edge(&origin, method, path, body, Some(&token)).await
}

fn normalize_play_info(info: &mut Value) {
    fn non_empty(v: Option<&Value>) -> Option<String> {
        v.and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
    let main = non_empty(info.get("streams").and_then(|s| s.get("main")));
    let hevc = non_empty(info.get("streams").and_then(|s| s.get("hevc")));
    let Some(obj) = info.as_object_mut() else { return };
    for (field, stream) in [("video_url", main), ("video_url_hevc", hevc)] {
        if let Some(url) = stream {
            if non_empty(obj.get(field)).is_none() {
                obj.insert(field.into(), json!(url));
            }
        }
    }
}

fn has_play_url(info: &Value) -> bool {
    info.get("video_url")
        .and_then(|v| v.as_str())
        .is_some_and(|x| !x.is_empty())
}

pub async fn dispatch(st: Arc<AppState>, cmd: &str, args: Value) -> Result<Value, String> {
    let a = if args.is_null() { json!({}) } else { args };
    match cmd {
        /* ── 会话 / 环境 ── */
        "app_boot" => {
            let origin = crate::net::base();
            Ok(json!({
                "user": st.api.session.user(&origin),
                "logged_in": st.api.session.token(&origin).is_some(),
                "base": origin,
                "strict_security": crate::network_policy::strict_security(),
                "cli_relaxed": crate::network_policy::cli_strict_override() == Some(false),
            }))
        }
        "debug_log" => {
            let msg = s(get(&a, "msg"));
            crate::logging::append_debug(&st.data_dir, &msg)?;
            Ok(Value::Null)
        }
        "net_diag" => Ok(crate::net::diagnose().await),
        "security_set" => {
            let strict = b(get(&a, "strict"));
            crate::network_policy::set_strict_security(strict)?;
            crate::net::refresh_client();
            crate::playback_log::record(
                "security.mode_changed",
                json!({"strict": crate::network_policy::strict_security()}),
            );
            Ok(json!(crate::network_policy::strict_security()))
        }

        /* ── 线路 ── */
        "line_state" => Ok(json!({
            "base": crate::net::base(),
            "default": crate::net::DEFAULT_BASE,
            "fixed": crate::lines::fixed(),
        })),
        "line_set" => {
            let previous = crate::net::base();
            let next = crate::net::set_base(&s(get(&a, "url")))?;
            if previous != next {
                st.api.session.clear();
            }
            Ok(json!(next))
        }
        "line_mirrors" => {
            crate::net::refresh_client();
            crate::lines::mirrors().await
        }
        "line_probe" => {
            crate::net::refresh_client();
            let urls: Vec<String> = a
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_str()).map(str::to_string).collect())
                .unwrap_or_default();
            Ok(crate::lines::probe(urls).await)
        }

        /* ── 更新 / 外壳 ── */
        "check_update" => {
            let current = env!("CARGO_PKG_VERSION").to_string();
            let url = "https://api.github.com/repos/Minireel/HGhub/releases/latest";
            let resp = crate::net::get(
                url,
                std::time::Duration::from_secs(12),
                Some("application/vnd.github+json"),
                None,
            )
            .await?;
            let status = resp.status();
            if status.as_u16() == 404 {
                return Ok(json!({
                    "current": current, "latest": "", "has_update": false,
                    "url": "https://github.com/Minireel/HGhub/releases",
                }));
            }
            if !status.is_success() {
                return Err(format!("GitHub 返回 HTTP {status}"));
            }
            let v: Value = resp.json().await.map_err(|e| crate::net::err_text(&e))?;
            let tag = v.get("tag_name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            Ok(json!({
                "current": current,
                "latest": tag.trim_start_matches(['v', 'V']).to_string(),
                "tag": tag,
                "has_update": false,
                "url": v.get("html_url").and_then(|x| x.as_str()).unwrap_or(""),
            }))
        }
        // 网页版没有系统 shell，链接由浏览器自己打开（前端 bridge 里已处理）
        "open_url" | "set_orientation" => Ok(Value::Null),

        /* ── 账号 ── */
        "api_login" => {
            let origin = crate::net::base();
            let data = crate::net::edge(
                &origin,
                "POST",
                "/api/account/loginByPassword",
                Some(json!({
                    "username": s(get(&a, "username")),
                    "password": s(get(&a, "password")),
                })),
                None,
            )
            .await?;
            let token = data
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "登录返回缺少 token".to_string())?
                .to_string();
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            if crate::net::base() != origin {
                return Err("登录期间线路已变化，请重新登录".into());
            }
            st.api.session.set(&origin, token, user.clone());
            Ok(json!({ "user": user }))
        }
        "api_register" => {
            let origin = crate::net::base();
            let password = s(get(&a, "password"));
            let email = s(get(&a, "email"));
            let mut body = json!({
                "username": s(get(&a, "username")),
                "password": password,
                "confirm_password": password,
            });
            if !email.trim().is_empty() {
                body["email"] = json!(email);
            }
            let data = crate::net::edge(
                &origin,
                "POST",
                "/api/account/registerByPassword",
                Some(body),
                None,
            )
            .await?;
            let token = data
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "注册返回缺少 token".to_string())?
                .to_string();
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            if crate::net::base() != origin {
                return Err("注册期间线路已变化，请重新登录".into());
            }
            st.api.session.set(&origin, token, user.clone());
            Ok(json!({ "user": user }))
        }
        "api_logout" => {
            let origin = crate::net::base();
            let token = st.api.session.token(&origin);
            let _ = crate::net::edge(&origin, "POST", "/api/account/logout", None, token.as_deref()).await;
            st.api.session.clear();
            Ok(json!({ "ok": true }))
        }
        "api_me" => {
            let origin = crate::net::base();
            let token = st.api.session.token(&origin);
            let me = crate::net::edge(&origin, "GET", "/api/account/me", None, token.as_deref()).await?;
            if crate::net::base() != origin {
                return Err("读取账号期间线路已变化".into());
            }
            if let Some(token) = st.api.session.token(&origin) {
                st.api.session.set(&origin, token, me.clone());
            }
            Ok(me)
        }

        /* ── 列表 ── */
        "api_feed" => {
            api_get(&st, &format!("/api/videos?page={}&size={}", u(get(&a, "page"), 1), u(get(&a, "size"), 20))).await
        }
        "api_category" => {
            api_get(&st, &format!(
                "/api/videos/category/{}?page={}&size={}",
                s(get(&a, "cate")),
                u(get(&a, "page"), 1),
                u(get(&a, "size"), 20)
            )).await
        }
        "api_rank" => {
            let kind = match s(get(&a, "kind")).as_str() {
                "recommend" => "recommend".to_string(),
                "potential" => "potential".to_string(),
                _ => "hot".to_string(),
            };
            api_get(&st, &format!("/api/ranks/{kind}")).await
        }
        "api_search" => {
            let q = s(get(&a, "q"));
            let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
            api_get(&st, &format!(
                "/api/search?q={encoded}&page={}&size={}",
                u(get(&a, "page"), 1),
                u(get(&a, "size"), 20)
            )).await
        }
        "api_posts" => {
            api_get(&st, &format!("/api/posts?page={}&size={}", u(get(&a, "page"), 1), u(get(&a, "size"), 20))).await
        }
        "api_post" => api_get(&st, &format!("/api/posts/{}", i(get(&a, "id")))).await,

        /* ── 详情 / 播放 ── */
        "api_video" => api_get(&st, &format!("/api/videos/{}", i(get(&a, "id")))).await,
        "api_play" => {
            let (id, ep) = (i(get(&a, "id")), u(get(&a, "ep"), 1));
            let primary = api_post(&st, &format!("/api/videos/{id}/episodes/{ep}/playback"), Some(json!({})))
                .await
                .map(|mut info| {
                    normalize_play_info(&mut info);
                    info
                });
            if let Ok(info) = &primary {
                if has_play_url(info) {
                    return primary;
                }
            }
            match api_get(&st, &format!("/api/videos/{id}/play?ep={ep}&page_texts=0")).await {
                Ok(mut legacy) => {
                    normalize_play_info(&mut legacy);
                    if has_play_url(&legacy) || primary.is_err() {
                        Ok(legacy)
                    } else {
                        primary
                    }
                }
                Err(_) => primary,
            }
        }

        /* ── 评论 / 互动 ── */
        "api_comments" => {
            api_get(&st, &format!(
                "/api/videos/{}/comments?ep={}&page={}&size={}",
                i(get(&a, "id")),
                u(get(&a, "ep"), 1),
                u(get(&a, "page"), 1),
                u(get(&a, "size"), 20)
            )).await
        }
        "api_comment_send" => {
            let (id, ep) = (i(get(&a, "id")), u(get(&a, "ep"), 1));
            let mut body = json!({ "text": s(get(&a, "text")) });
            let reply = get2(&a, "reply_id", "replyId").and_then(|x| x.as_i64());
            let path = match reply {
                Some(rid) if rid > 0 => {
                    body["reply_id"] = json!(rid);
                    format!("/api/videos/{id}/comments")
                }
                _ => format!("/api/videos/{id}/comments?ep={ep}"),
            };
            api_authed(&st, "POST", &path, Some(body)).await
        }
        "api_like" => {
            let on = b(get(&a, "on"));
            api_authed(
                &st,
                if on { "POST" } else { "DELETE" },
                &format!("/api/videos/{}/like?ep={}", i(get(&a, "id")), u(get(&a, "ep"), 1)),
                None,
            ).await
        }
        "api_collect" => {
            let on = b(get(&a, "on"));
            api_authed(
                &st,
                if on { "POST" } else { "DELETE" },
                &format!("/api/videos/{}/collect", i(get(&a, "id"))),
                None,
            ).await
        }
        "api_follow" => {
            let on = b(get(&a, "on"));
            api_authed(
                &st,
                if on { "POST" } else { "DELETE" },
                &format!("/api/users/{}/follow", i(get(&a, "uid"))),
                None,
            ).await
        }
        "api_post_like" => {
            let on = b(get(&a, "on"));
            api_authed(
                &st,
                if on { "POST" } else { "DELETE" },
                &format!("/api/posts/{}/like", i(get(&a, "id"))),
                None,
            ).await
        }
        "api_post_collect" => {
            let on = b(get(&a, "on"));
            api_authed(
                &st,
                if on { "POST" } else { "DELETE" },
                &format!("/api/posts/{}/collect", i(get(&a, "id"))),
                None,
            ).await
        }
        "api_post_comments" => {
            api_get(&st, &format!(
                "/api/posts/{}/comments?page={}&size={}",
                i(get(&a, "id")),
                u(get(&a, "page"), 1),
                u(get(&a, "size"), 20)
            )).await
        }

        /* ── 缓存 / 诊断 ── */
        "cache_clear" => {
            crate::media::clear_caches();
            Ok(Value::Null)
        }
        "playback_log_status" => Ok(serde_json::to_value(crate::playback_log::playback_log_status()?).map_err(|e| e.to_string())?),
        "playback_log_set" => {
            let on = b(get(&a, "enabled"));
            Ok(serde_json::to_value(crate::playback_log::playback_log_set(on)?).map_err(|e| e.to_string())?)
        }
        "playback_log_write" => {
            let entries = a.get("entries").cloned().unwrap_or(json!([]));
            let parsed: Vec<crate::playback_log::Entry> =
                serde_json::from_value(entries).map_err(|e| e.to_string())?;
            crate::playback_log::playback_log_write(parsed)?;
            Ok(Value::Null)
        }
        "playback_log_read" => Ok(json!(crate::playback_log::playback_log_read()?)),

        other => Err(format!("未知命令：{other}")),
    }
}

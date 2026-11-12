import json, sys, urllib.parse, urllib.request, urllib.error

BASE = "http://127.0.0.1:8900"
PW = open(r"C:\Code\Hg\web\.web-password", encoding="ascii").read()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


plain = urllib.request.build_opener(NoRedirect)
plain.addheaders = [("User-Agent", "hgweb-verify/1.0")]


def call(path, method="GET", body=None, cookie=None, raw=False):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if cookie:
        headers["Cookie"] = cookie
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with plain.open(req, timeout=90) as resp:
            payload = resp.read()
            return resp.status, resp.headers, (payload if raw else payload.decode(errors="replace"))
    except urllib.error.HTTPError as e:
        return e.code, e.headers, e.read().decode(errors="replace")


results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    print(("PASS  " if ok else "FAIL  ") + name + (("   [" + detail + "]") if detail else ""))


# ── 1. 未登录 ───────────────────────────────────────────────
st, hd, _ = call("/")
check("外网未登录访问首页被挡回登录页", st in (302, 303, 307) and hd.get("Location") == "/login",
      f"status={st} location={hd.get('Location')}")
st, _, _ = call("/rpc/app_boot", "POST", {})
check("未登录 RPC 返回 401", st == 401, f"status={st}")
st, _, _ = call("/media/img?u=" + urllib.parse.quote("https://example.com/a.jpg", safe=""))
check("未登录媒体代理返回 401", st == 401, f"status={st}")
st, _, _ = call("/assets/", "GET")
check("未登录静态资源也被挡", st in (302, 303, 307, 401, 404), f"status={st}")

# ── 2. 登录 ─────────────────────────────────────────────────
st, hd, body = call("/login", "POST", {"password": PW})
cookie = ""
for k, v in hd.items():
    if k.lower() == "set-cookie":
        cookie = v.split(";")[0]
check("口令登录成功并下发 Cookie", st == 200 and cookie.startswith("hgweb=v1."),
      f"status={st} cookie={'有' if cookie else '无'}")

st, _, body = call("/", cookie=cookie)
check("登录后首页返回前端入口", st == 200 and 'id="root"' in body, f"status={st} bytes={len(body)}")

st, _, body = call("/rpc/app_boot", "POST", {}, cookie=cookie)
boot = json.loads(body) if body.strip().startswith("{") else {}
check("app_boot 可用（线路基址）", bool(boot.get("ok")), f"base={boot.get('data', {}).get('base')}")

# ── 3. feed 真实数据 ────────────────────────────────────────
st, _, body = call("/rpc/api_feed", "POST", {"page": 1, "size": 6}, cookie=cookie)
feed = json.loads(body) if body.strip().startswith("{") else {}
data = feed.get("data") or {}
items = data.get("items") if isinstance(data, dict) else data
if isinstance(items, dict):
    items = items.get("items") or []
items = items or []
check("登录后首页 feed 是真实数据", len(items) > 0, f"items={len(items)}")
first = items[0] if items else {}
vid = first.get("id") or first.get("video_id") or first.get("vid")
cover_raw = first.get("cover") or first.get("thumb") or first.get("pic") or first.get("cover_url") or ""
check("feed 首条含 id / 封面字段", bool(vid) and bool(cover_raw),
      f"id={vid} title={str(first.get('title') or first.get('name') or '')[:20]}")

# ── 4. 详情 + 播放 ──────────────────────────────────────────
play_url = ""
if vid:
    st, _, body = call("/rpc/api_video", "POST", {"id": vid}, cookie=cookie)
    detail = json.loads(body) if body.strip().startswith("{") else {}
    check("任选一部剧详情能打开", bool(detail.get("ok")) and isinstance(detail.get("data"), (dict, list)),
          f"status={st} keys={list((detail.get('data') or {}))[:5] if isinstance(detail.get('data'), dict) else 'list'}")

    st, _, body = call("/rpc/api_play", "POST", {"id": vid, "ep": 1}, cookie=cookie)
    play = json.loads(body) if body.strip().startswith("{") else {}
    play_url = (play.get("data") or {}).get("video_url") or ""
    check("取到可播放地址", bool(play_url), f"{play_url[:64]}")

# ── 5. HLS 清单 + 分片 ─────────────────────────────────────
if play_url:
    st, hd, text = call("/media/hls?u=" + urllib.parse.quote(play_url, safe=""), cookie=cookie)
    check("HLS 清单经代理返回 200", st == 200 and text.lstrip().startswith("#EXTM3U"),
          f"status={st} ctype={hd.get('Content-Type')}")
    segs = [l.strip() for l in text.splitlines() if l.strip() and not l.startswith("#") and "/seg?u=" in l]
    check("清单内分片已改写为同源代理地址", len(segs) > 0, f"seg_lines={len(segs)}")
    if segs:
        rel = segs[0].split("127.0.0.1:8900")[-1] if segs[0].startswith("http") else segs[0]
        if rel.startswith("/media"):
            st, hd, raw = call(rel, cookie=cookie, raw=True)
        else:
            st, hd, raw = call(rel, cookie=cookie, raw=True)
        check("视频分片经代理返回 200", st == 200 and len(raw) > 1000,
              f"status={st} bytes={len(raw)} ctype={hd.get('Content-Type')}")

# ── 6. 封面 ─────────────────────────────────────────────────
if cover_raw:
    st, hd, raw = call("/media/img?u=" + urllib.parse.quote(cover_raw, safe=""), cookie=cookie, raw=True)
    ctype = hd.get("Content-Type", "")
    check("封面经代理解密加载", st == 200 and ctype.startswith("image/") and len(raw) > 3000,
          f"status={st} bytes={len(raw)} ctype={ctype}")

print("\n=== 汇总: %d/%d 通过 ===" % (sum(1 for _, ok in results if ok), len(results)))
sys.exit(0 if all(ok for _, ok in results) else 1)

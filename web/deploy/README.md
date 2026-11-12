# HGhub 网页版部署包（Linux 服务器）

服务：单个静态二进制 `hghub-web`，自带前端静态托管、RPC、媒体代理、口令登录门。
依赖：无（Rust 静态链接，不装 Node、不装 Rust、不依赖 nginx）。

目录约定：

```
/opt/hghub-web/hghub-web        # 二进制
/opt/hghub-web/dist/            # 前端产物（vite build 结果）
/etc/hghub-web/env              # 环境变量（含口令，chmod 600）
/var/lib/hghub-web/             # 运行时数据：session.json / auth-salt.bin / 线路与日志
```

`/etc/hghub-web/env` 内容：

```
HGHUB_PASSWORD=这里换成你的访问口令（≥6 位）
HGHUB_BIND=127.0.0.1:8900
HGHUB_DIST=/opt/hghub-web/dist
HGHUB_DATA=/var/lib/hghub-web
```

## 上线顺序（不要反）

1. **只绑本机**跑起来：`systemctl start hghub-web`，`ss -ltnp | grep 8900` 应显示 `127.0.0.1:8900`。
2. 在服务器本地验通：`curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8900/` 得 `303`（未登录跳登录页），
   `curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8900/login` 得 `200`。
3. 需要外网访问时再改 `HGHUB_BIND=0.0.0.0:8900` 并**去云控制台安全组放行 8900**（两处都要开：系统防火墙 + 云安全组）。
4. 外网复验四项：未登录被挡、登录后 feed 有真实数据、详情能开、HLS 清单与分片经代理 200、封面能加载。

## 开机自启

```
systemctl enable --now hghub-web
```

## 常见问题

- 页面能开但卡片空白：服务器到上游线路不通。用 `journalctl -u hghub-web -n 100` 看 `network.target_failed`，
  然后在 App 的「设置 → 线路」换线（服务端同样生效，线路会落盘到 `/var/lib/hghub-web`）。
- 想看结构化播放日志：`curl` 调 RPC `playback_log_set` 打开后，日志在 `/var/lib/hghub-web/playback-*.jsonl`。
- 口令泄漏：改 `/etc/hghub-web/env` 里的 `HGHUB_PASSWORD` 后 `systemctl restart hghub-web`，
  已登录会话立即失效（Cookie 签名密钥由口令派生）。

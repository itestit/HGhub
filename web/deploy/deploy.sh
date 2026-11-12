#!/usr/bin/env bash
# 一次性安装/更新 HGhub 网页版。默认只绑 127.0.0.1，验通后再自己决定是否对外。
# 用法：sudo bash deploy.sh /path/to/hghub-web_1.0.0_linux_x86_64
set -euo pipefail

SRC="${1:?用法: sudo bash deploy.sh <解压后的目录>}"
APP=/opt/hghub-web
DATA=/var/lib/hghub-web
ENVF=/etc/hghub-web/env

command -v systemctl >/dev/null || { echo "需要 systemd"; exit 1; }
[ -x "$SRC/hghub-web" ] || { echo "$SRC 里没有 hghub-web"; exit 1; }
[ -f "$SRC/dist/index.html" ] || { echo "$SRC 里没有 dist/index.html"; exit 1; }

id hghub >/dev/null 2>&1 || useradd -r -s /usr/sbin/nologin hghub

install -d -m 0755 "$APP" "$APP/dist"
install -m 0755 "$SRC/hghub-web" "$APP/hghub-web"
cp -r "$SRC/dist/." "$APP/dist/"
install -d -m 0750 -o hghub -g hghub "$DATA"

if [ ! -f "$ENVF" ]; then
  install -d -m 0750 /etc/hghub-web
  PW="$(head -c 18 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 18)"
  cat > "$ENVF" <<EOF
HGHUB_PASSWORD=$PW
HGHUB_BIND=127.0.0.1:8900
HGHUB_DIST=$APP/dist
HGHUB_DATA=$DATA
EOF
  chmod 600 "$ENVF"
  echo "已生成访问口令，写在 $ENVF（HGHUB_PASSWORD），请自行抄走并妥善保存"
else
  echo "沿用已有 $ENVF"
fi

install -m 0644 "$SRC/deploy/hghub-web.service" /etc/systemd/system/hghub-web.service
systemctl daemon-reload
systemctl enable --now hghub-web
sleep 2
systemctl --no-pager --lines=5 status hghub-web || true

echo
echo "本地自检（此时只监听 127.0.0.1）："
echo -n "  GET /       -> "; curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8900/
echo -n "  GET /login  -> "; curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8900/login
cat <<'EOT'

本地验通后，若要外网访问，两处都要开：
  1) /etc/hghub-web/env 改成 HGHUB_BIND=0.0.0.0:8900 && systemctl restart hghub-web
  2) 云控制台安全组 + 本机防火墙放行 8900
再复验：未登录被挡、登录后 feed 有数据、详情能开、HLS 清单与分片 200、封面能加载。
EOT

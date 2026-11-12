# 从 HGhub 仓库同步 Rust 核心模块到 web/server/src，并应用最小补丁。
# 复用（而不是重写）的数据/解密/流转发逻辑都在这些文件里：
#   net.rs            上游请求 + 线路 + 代理 + 错误分类
#   media.rs          /img 封面解密、/hls 清单改写、/seg 分片转发
#   lines.rs          固定线路与镜像线路
#   session.rs        上游账号 token 存储
#   network_policy.rs 严格安全模式与 URL 白名单
#   logging.rs        调试日志落盘
#   playback_log.rs   结构化播放日志
param(
  [string]$Repo = "C:\Code\Hg\HGhub",
  [string]$Dest = "$PSScriptRoot\..\src"
)
$ErrorActionPreference = 'Stop'
$src = Join-Path $Repo "src-tauri\src"
$files = @('net.rs','media.rs','lines.rs','session.rs','network_policy.rs','logging.rs','playback_log.rs')
New-Item -ItemType Directory -Force $Dest | Out-Null
foreach ($f in $files) {
  $text = [IO.File]::ReadAllText((Join-Path $src $f), [Text.Encoding]::UTF8)
  switch ($f) {
    'media.rs' {
      # tauri::http 就是 http crate 的再导出；服务端直接用 http crate
      $text = $text -replace 'use tauri::http::\{Response, StatusCode\};', 'use http::{Response, StatusCode};'
    }
    'playback_log.rs' {
      # 去掉 tauri 命令宏属性，其余逻辑（含脱敏）原样保留
      $text = [regex]::Replace($text, '(?m)^\s*#\[tauri::command\]\s*$\r?\n', '')
    }
  }
  [IO.File]::WriteAllText((Join-Path $Dest $f), $text, (New-Object Text.UTF8Encoding($false)))
  "vendored {0}" -f $f
}
# 补丁自检：确认没有任何 tauri 符号残留
$bad = Select-String -Path (Join-Path $Dest '*.rs') -Pattern 'tauri::|use tauri'
if ($bad) { $bad | ForEach-Object { "STILL TAURI: $($_.Filename):$($_.LineNumber) $($_.Line.Trim())" }; exit 1 }
"vendor OK: 0 tauri references"

#!/usr/bin/env pwsh
# 把桌面端前端加工成网页版：只做「invoke → fetch」和「hg:// → /media」两件事，业务代码不动。
# 用法：.\apply-frontend-patch.ps1 -AppDir C:\path\to\app-copy
param([Parameter(Mandatory=$true)][string]$AppDir)
$ErrorActionPreference = 'Stop'
$src = Join-Path $AppDir 'src'
$bridge = Join-Path $PSScriptRoot 'bridge.ts'
Copy-Item $bridge (Join-Path $src 'bridge.ts') -Force

# 1) 统一入口：所有 invoke 都走 bridge
Get-ChildItem $src -Recurse -File -Include *.ts,*.tsx | Where-Object { $_.Name -ne 'bridge.ts' } | ForEach-Object {
  $rel  = $_.FullName.Substring($src.Length).TrimStart('\')
  $up   = '../' * (($rel -split '\\').Count - 1)
  $text = [IO.File]::ReadAllText($_.FullName, [Text.Encoding]::UTF8)
  $text = $text -replace 'import \{ invoke \} from "@tauri-apps/api/core";', "import { invoke } from `"$($up)bridge`";"
  $text = $text -replace 'import \{ addPluginListener, invoke \} from "@tauri-apps/api/core";', "import { addPluginListener, invoke } from `"$($up)bridge`";"
  $text = $text -replace 'import \{ getCurrentWindow \} from "@tauri-apps/api/window";', "import { getCurrentWindow } from `"$($up)bridge`";"
  [IO.File]::WriteAllText($_.FullName, $text, (New-Object Text.UTF8Encoding($false)))
}

# 2) 媒体协议：桌面端 hg://localhost 换成同源 /media 前缀
$api = Join-Path $src 'api.ts'
$t = [IO.File]::ReadAllText($api, [Text.Encoding]::UTF8)
$t = $t -replace 'import \{ invoke \} from "\./bridge";', 'import { invoke, isTauri } from "./bridge";'
$t = $t -replace 'export const LOCAL = isMacos \|\| isIos \|\| isLinux \? "hg://localhost" : "http://hg.localhost";',
@'
export const LOCAL = isTauri
  ? (isMacos || isIos || isLinux ? "hg://localhost" : "http://hg.localhost")
  : "/media";
'@
[IO.File]::WriteAllText($api, $t, (New-Object Text.UTF8Encoding($false)))

Write-Host "前端补丁已应用：$(Get-ChildItem $src -Recurse -File -Include *.ts,*.tsx | Measure-Object | Select-Object -ExpandProperty Count) 个源文件"

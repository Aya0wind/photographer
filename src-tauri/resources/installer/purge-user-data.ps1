# Photo Hub 用户数据清除（安装器「纯净安装」与「卸载删数据」共用）。
# 语义（用户 2026-09-29 定义）：
#   删除：应用配置目录（%APPDATA%\com.smartphoto.app，含 settings.json 与
#         已下载的 AI 模型缓存）+ 各图库的 dbDir（数据库/缩略图/向量/标记）。
#   保留：照片存储根（photoRoot）——绝不删除任何照片文件。
# 安全闸：dbDir 位于 photoRoot 内部或与 photoRoot 相同时跳过该库的清除
#         （宁可漏删数据库，绝不误删照片）。

$ErrorActionPreference = 'Stop'
$config = Join-Path $env:APPDATA 'com.smartphoto.app'
$settings = Join-Path $config 'settings.json'
$purged = @()

if (Test-Path -LiteralPath $settings) {
    try {
        $json = Get-Content -LiteralPath $settings -Raw | ConvertFrom-Json
        foreach ($lib in @($json.libraries)) {
            $dbDir = $lib.dbDir
            $photoRoot = $lib.photoRoot
            if (-not $dbDir -or -not (Test-Path -LiteralPath $dbDir)) { continue }

            # 根路径形态归一（尾反斜杠/大小写不敏感比较都用前缀语义）
            $dbFull = [System.IO.Path]::GetFullPath($dbDir).TrimEnd('\')
            if ($photoRoot) {
                $photoFull = [System.IO.Path]::GetFullPath($photoRoot).TrimEnd('\')
                # dbDir == photoRoot，或 dbDir 在 photoRoot 之内 → 一律不动（照片在下面）
                if ($dbFull -ieq $photoFull -or $dbFull.StartsWith($photoFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
                    Write-Host "SKIP (inside photoRoot): $dbDir"
                    continue
                }
            }
            Remove-Item -LiteralPath $dbDir -Recurse -Force
            $purged += $dbDir
            Write-Host "PURGED dbDir: $dbDir"
        }
    }
    catch {
        Write-Host "settings.json parse failed: $($_.Exception.Message) @ $($_.InvocationInfo.PositionMessage)"
    }
}

if (Test-Path -LiteralPath $config) {
    Remove-Item -LiteralPath $config -Recurse -Force
    $purged += $config
    Write-Host "PURGED config: $config"
}

Write-Host ("Photo Hub purge done. Items: " + $(if ($purged.Count) { $purged -join '; ' } else { '(none)' }))

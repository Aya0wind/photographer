# Photo Hub 用户数据清除（安装器「纯净安装」与「卸载删数据」共用）。
# 语义（2026-10-09 多数据库修正）：清应用配置目录（%APPDATA%\com.smartphoto.app：
#   settings.json + 默认约定路径下的各数据库——SQLite/缩略图/向量/人脸）
#   **另加遍历** settings.json 里各 databases[].dbDir 的自定义数据目录
#   （Windows 数据盘上的库）。
# 安全闸（用户红线——绝不删照片库文件夹）：
#   - 删任一自定义 dbDir 前收集**所有**数据库 library.db 的 photos_libraries
#     根目录（读不开 → SKIP 该 dbDir，宁可漏删不可误删照片）；
#   - dbDir 与任何照片库根相同或互相包含 → SKIP；
#   - 照片库根永远在用户自选的文件夹，登记/移除都不搬动文件。
# SQLite 读取依赖 sqlite3 CLI（PATH 可见）；不可用时全部自定义 dbDir 按
# 「读不开」处理输出 SKIP（保守安全），配置目录照清。

$ErrorActionPreference = 'Stop'
$config = Join-Path $env:APPDATA 'com.smartphoto.app'

# --- 收集照片库根目录（全部数据库的 photos_libraries；$null=无法完整核对） ---
function Get-PhotoRoots {
    param([array]$Databases)
    $roots = @()
    foreach ($db in $Databases) {
        $libraryDb = Join-Path $db.dbDir 'library.db'
        if (-not (Test-Path -LiteralPath $libraryDb)) { continue }
        $sqlite = Get-Command sqlite3 -ErrorAction SilentlyContinue
        if ($null -eq $sqlite) { return $null }   # 无读取工具：无法核对
        $output = & $sqlite.Source $libraryDb "SELECT root_path FROM photos_libraries;" 2>$null
        if ($LASTEXITCODE -ne 0) { return $null } # 读不开：无法核对
        foreach ($line in @($output)) {
            if ($line) { $roots += [string]$line }
        }
    }
    return ,@($roots)
}

# 两路径「相同或互相包含」（忽略大小写与分隔符方向；前缀后必须是分隔符）
function Test-PathOverlap {
    param([string]$A, [string]$B)
    $a = ($A -replace '/', '\').TrimEnd('\').ToLower()
    $b = ($B -replace '/', '\').TrimEnd('\').ToLower()
    return $a.StartsWith($b + '\') -or $b.StartsWith($a + '\') -or $a -eq $b
}

# 1) 清应用配置目录（含默认约定路径 databases/ 下的数据库与 settings.json）
$settingsFile = Join-Path $config 'settings.json'
$customDatabases = @()
if (Test-Path -LiteralPath $settingsFile) {
    try {
        $parsed = Get-Content -LiteralPath $settingsFile -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($parsed.databases) { $customDatabases = @($parsed.databases) }
    } catch {
        Write-Host "WARN settings.json unreadable; custom database dirs not traversed."
    }
}
if (Test-Path -LiteralPath $config) {
    # 配置目录里的照片库根重叠保护：配置目录整删前也过同一安全闸
    $roots = Get-PhotoRoots -Databases $customDatabases
    $configOverlap = $false
    if ($null -ne $roots) {
        foreach ($root in $roots) {
            if (Test-PathOverlap -A $config -B $root) {
                Write-Host "SKIP (overlaps photo library root): $config"
                $configOverlap = $true
                break
            }
        }
    }
    if (-not $configOverlap) {
        Remove-Item -LiteralPath $config -Recurse -Force
        Write-Host "PURGED app data: $config"
    }
} else {
    Write-Host "SKIP (not found): $config"
}

# 2) 遍历自定义 dbDir（照片库根重叠 / library.db 读不开 → SKIP，绝不误删照片）
$photoRoots = Get-PhotoRoots -Databases $customDatabases
foreach ($db in $customDatabases) {
    $dir = [string]$db.dbDir
    if ($dir -and $dir.StartsWith($config, [System.StringComparison]::OrdinalIgnoreCase)) {
        continue  # 已随配置目录处理
    }
    if (-not (Test-Path -LiteralPath $dir)) {
        Write-Host "SKIP (not found): $dir"
        continue
    }
    if ($null -eq $photoRoots) {
        Write-Host "SKIP (photo roots unverifiable): $dir"
        continue
    }
    $overlap = $false
    foreach ($root in $photoRoots) {
        if (Test-PathOverlap -A $dir -B $root) { $overlap = $true; break }
    }
    if ($overlap) {
        Write-Host "SKIP (overlaps photo library root): $dir"
        continue
    }
    Remove-Item -LiteralPath $dir -Recurse -Force
    Write-Host "PURGED database dir: $dir"
}

Write-Host "Photo Hub purge done. Photo library folders untouched."

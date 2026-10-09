# Photo Hub 用户数据清除（安装器「纯净安装」与「卸载删数据」共用）。
# 语义（2026-10-09 单数据库多照片库定案）：只清应用数据目录
#   （%APPDATA%\com.smartphoto.app：settings.json + 应用级唯一数据库——
#   SQLite/缩略图/向量/人脸，未自定义「数据库位置」时全在其中）。
# 保留：所有照片库根文件夹——照片库根永远在用户自选的文件夹，登记/移除都
#   不搬动文件，不存在「数据库混在照片目录里」的形态，旧 dbDir-vs-photoRoot
#   安全闸天然成立，无需再逐库比对路径。

$ErrorActionPreference = 'Stop'
$config = Join-Path $env:APPDATA 'com.smartphoto.app'

if (Test-Path -LiteralPath $config) {
    Remove-Item -LiteralPath $config -Recurse -Force
    Write-Host "PURGED app data: $config"
} else {
    Write-Host "SKIP (not found): $config"
}

Write-Host "Photo Hub purge done. App data directory only; photo library folders untouched."

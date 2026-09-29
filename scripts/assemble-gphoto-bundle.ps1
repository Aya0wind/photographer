# 从 MSYS2 ucrt64 组装 libgphoto2 随包目录（src-tauri/resources/gphoto/）。
# 打包前运行（tauri build 收取该目录进安装包；LGPL-2.1 动态链接，随包
# 附 libgphoto2 源码链接即合规——项目开源）。
#
# 用法: powershell -File scripts\assemble-gphoto-bundle.ps1 [-MsysRoot C:\msys64]
param(
    [string]$MsysRoot = "C:\msys64"
)

$ErrorActionPreference = "Stop"
$UcrtBin = Join-Path $MsysRoot "ucrt64\bin"
$UcrtLib = Join-Path $MsysRoot "ucrt64\lib"
$OutDir = Join-Path $PSScriptRoot "..\src-tauri\resources\gphoto"

if (-not (Test-Path (Join-Path $UcrtBin "libgphoto2-6.dll"))) {
    Write-Error "libgphoto2 未安装：$UcrtBin\libgphoto2-6.dll 不存在（pacman -S mingw-w64-ucrt-x86_64-libgphoto2）"
}

# 主库 + port 库 + 静态导入闭包 + ptp2/usb1 运行期依赖（objdump -p 实测，
# 与 gphoto_backend.rs preload_deps 清单保持一致）
$dlls = @(
    "libgphoto2-6.dll", "libgphoto2_port-12.dll",
    "libwinpthread-1.dll", "libiconv-2.dll", "libtre-5.dll", "zlib1.dll",
    "libusb-1.0.dll", "libexif-12.dll", "libintl-8.dll", "libsystre-0.dll",
    "libjpeg-8.dll", "libxml2-16.dll", "libltdl-7.dll"
)

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
foreach ($dll in $dlls) {
    Copy-Item (Join-Path $UcrtBin $dll) -Destination $OutDir -Force
}

# iolibs / camlibs 整目录（运行期 ltdl 加载的驱动；libgphoto2 按编译期
# 前缀找它们——随包布局靠 IOLIBS/CAMLIBS 环境变量重定位，应用启动时注入）
$iolibs = Get-ChildItem (Join-Path $UcrtLib "libgphoto2_port") -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName "usb1.dll") } |
    Select-Object -First 1
$camlibs = Get-ChildItem (Join-Path $UcrtLib "libgphoto2") -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName "ptp2.dll") } |
    Select-Object -First 1
if (-not $iolibs) { Write-Error "未找到 iolibs（usb1.dll）于 $UcrtLib\libgphoto2_port" }
if (-not $camlibs) { Write-Error "未找到 camlibs（ptp2.dll）于 $UcrtLib\libgphoto2" }

Copy-Item $iolibs.FullName -Destination (Join-Path $OutDir "iolibs") -Recurse -Force
Copy-Item $camlibs.FullName -Destination (Join-Path $OutDir "camlibs") -Recurse -Force

$count = (Get-ChildItem $OutDir -Recurse -File).Count
Write-Host "完成：$OutDir（$count 个文件；DLL x$($dlls.Count) + iolibs + camlibs）"

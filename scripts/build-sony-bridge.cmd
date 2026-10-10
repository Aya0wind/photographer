@echo off
REM ============================================================
REM  Sony 联机拍摄桥构建脚本（阶段 E）
REM  前置：Sony Camera Remote SDK（官网表单申请下载）解压出的
REM        RemoteCli 目录路径作为第一个参数（或 SONY_SDK_ROOT 环境变量）。
REM  产物：%LOCALAPPDATA%\Photographer\sony-sdk\bridge-build\Release\photographer-sony.exe
REM        （后端 helper_path 的默认探测位置；SDK 的 crsdk DLL 会随构建拷贝到产物旁）
REM  用法：scripts\build-sony-bridge.cmd C:\path\to\RemoteCli
REM ============================================================
setlocal enabledelayedexpansion
cd /d "%~dp0.."

set "SDK_ROOT=%~1"
if "%SDK_ROOT%"=="" set "SDK_ROOT=%SONY_SDK_ROOT%"
if "%SDK_ROOT%"=="" (
  echo [FAIL] 用法: scripts\build-sony-bridge.cmd ^<RemoteCli 目录^>  （或先 set SONY_SDK_ROOT）
  goto :fail
)
if not exist "%SDK_ROOT%\app\CRSDK\CameraRemote_SDK.h" (
  echo [FAIL] %SDK_ROOT% 不像 RemoteCli 目录（缺 app\CRSDK\CameraRemote_SDK.h）
  goto :fail
)
where cmake >nul 2>nul || (echo [FAIL] cmake not found in PATH & goto :fail)

set "OUT=%LOCALAPPDATA%\Photographer\sony-sdk\bridge-build"
if not exist "%OUT%" mkdir "%OUT%"

echo [1/2] CMake configure（SONY_SDK_ROOT=%SDK_ROOT%）...
cmake -S src-tauri\sony-bridge -B "%OUT%" -G "Visual Studio 17 2022" -A x64 -DSONY_SDK_ROOT="%SDK_ROOT%" || goto :fail

echo [2/2] Build Release...
cmake --build "%OUT%" --config Release || goto :fail

if not exist "%OUT%\Release\photographer-sony.exe" (
  echo [FAIL] 构建完成但未找到 photographer-sony.exe
  goto :fail
)
echo.
echo DONE. 桥已就绪：%OUT%\Release\photographer-sony.exe
echo （应用无需重启：后端每次连接时按路径探测该 exe）
goto :eof

:fail
echo [BUILD FAILED]
exit /b 1

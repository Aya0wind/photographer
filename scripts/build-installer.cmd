@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion
REM ============================================================
REM  Smart Photo Windows 安装包构建脚本
REM  产物: NSIS exe (currentUser 免管理员) + WiX msi 双格式
REM  输出: installer-output\ 目录
REM  用法: scripts\build-installer.cmd [--nopause]
REM  说明: 首次打包会自动下载 NSIS/WiX 工具链到 %LOCALAPPDATA%\tauri
REM ============================================================
set PATH=%USERPROFILE%\.local\nodejs;%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0.."

echo [1/4] 检查工具链...
where node >nul 2>nul || (echo [失败] 未找到 node（预期 %USERPROFILE%\.local\nodejs）& goto :fail)
where cargo >nul 2>nul || (echo [失败] 未找到 cargo（预期 %USERPROFILE%\.cargo\bin）& goto :fail)

echo [2/4] 前端依赖...
if not exist node_modules (
  call npm install --no-audit --no-fund || goto :fail
) else (
  echo        node_modules 已存在，跳过
)

echo [3/4] Tauri 构建（release 编译 + NSIS exe + WiX msi）...
call npm run tauri build || goto :fail

echo [4/4] 收集产物...
set OUTDIR=installer-output
if not exist %OUTDIR% mkdir %OUTDIR%
set FOUND=0
for %%F in ("src-tauri\target\release\bundle\nsis\*-setup.exe") do (
  copy /y "%%~fF" "%OUTDIR%\" >nul && set FOUND=1
  echo        exe: %%~nxF
)
for %%F in ("src-tauri\target\release\bundle\msi\*.msi") do (
  copy /y "%%~fF" "%OUTDIR%\" >nul && set FOUND=1
  echo        msi: %%~nxF
)
if %FOUND%==0 (
  echo [失败] 未找到任何安装包产物
  goto :fail
)
echo.
echo 完成！安装包在 %CD%\%OUTDIR%\
dir /b %OUTDIR%

if not "%1"=="--nopause" pause
exit /b 0

:fail
echo.
echo [构建失败]
if not "%1"=="--nopause" pause
exit /b 1

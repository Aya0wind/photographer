@echo off
REM ============================================================
REM  Smart Photo Windows installer build script
REM  Output: two NSIS exe variants (both currentUser, no admin):
REM   <product>_<ver>_x64-setup.exe                  - online: webviewInstallMode=downloadBootstrapper,
REM                                                   auto-downloads WebView2 during install if missing
REM   <product>_<ver>_x64-webview2-offline-setup.exe - offline: bundles the full WebView2 runtime
REM                                                   (~+127MB, installs fully offline)
REM  Artifacts: installer-output\
REM  Usage: scripts\build-installer.cmd [--nopause]
REM  Notes: first run downloads NSIS toolchain to %LOCALAPPDATA%\tauri
REM ============================================================
setlocal enabledelayedexpansion
set PATH=%USERPROFILE%\.local\nodejs;%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0.."

REM flags may appear in any order (CI passes --nopause --online-only)
set ONLINE_ONLY=0
set NOPAUSE=0
for %%A in (%*) do (
  if /i "%%A"=="--online-only" set ONLINE_ONLY=1
  if /i "%%A"=="--nopause" set NOPAUSE=1
)

echo [1/5] toolchain check...
where node >nul 2>nul || (echo [FAIL] node not found, expected %USERPROFILE%\.local\nodejs & goto :fail)
where cargo >nul 2>nul || (echo [FAIL] cargo not found, expected %USERPROFILE%\.cargo\bin & goto :fail)

echo [2/5] frontend deps...
if not exist node_modules call npm install --no-audit --no-fund || goto :fail

set OUTDIR=installer-output
if exist %OUTDIR% rmdir /s /q %OUTDIR%
if not exist %OUTDIR% mkdir %OUTDIR%

echo [3/5] Tauri build - online bootstrapper (default config)...
call npm run tauri build || goto :fail
set FOUND=0
for %%F in ("src-tauri\target\release\bundle\nsis\*-setup.exe") do (
  copy /y "%%~fF" "%OUTDIR%\" >nul && set FOUND=1
  echo        exe online: %%~nxF
)

if %ONLINE_ONLY%==1 goto skip_offline
echo [4/5] Tauri build - offline WebView2 (config overlay, incremental)...
call npm run tauri build -- --config src-tauri\tauri.webview-offline.conf.json || goto :fail
for %%F in ("src-tauri\target\release\bundle\nsis\*-setup.exe") do (
  copy /y "%%~fF" "%OUTDIR%\%%~nF-webview2-offline%%~xF" >nul && set FOUND=1
  echo        exe offline: %%~nF-webview2-offline%%~xF
)

:skip_offline

echo [5/5] verify artifacts...
if %FOUND%==0 (
  echo [FAIL] no installer artifacts found
  goto :fail
)
echo.
echo DONE. Installers at %CD%\%OUTDIR%\
dir /b %OUTDIR%

if not %NOPAUSE%==1 pause
exit /b 0

:fail
echo.
echo [BUILD FAILED]
if not %NOPAUSE%==1 pause
exit /b 1

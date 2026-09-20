@echo off
REM ============================================================
REM  Smart Photo Windows installer build script
REM  Output: NSIS exe (currentUser, no admin) + WiX msi
REM  Artifacts: installer-output\
REM  Usage: scripts\build-installer.cmd [--nopause]
REM  Notes: first run downloads NSIS/WiX toolchain to %LOCALAPPDATA%\tauri
REM         and ffmpeg sidecar (gyan.dev release-essentials) if missing
REM  IMPORTANT: keep this file ASCII-only and keep the ffmpeg download
REM  logic in a subroutine (parsing parenthesized blocks with redirects
REM  inside is fragile in cmd.exe).
REM ============================================================
setlocal enabledelayedexpansion
set PATH=%USERPROFILE%\.local\nodejs;%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0.."

echo [1/5] toolchain check...
where node >nul 2>nul || (echo [FAIL] node not found, expected %USERPROFILE%\.local\nodejs & goto :fail)
where cargo >nul 2>nul || (echo [FAIL] cargo not found, expected %USERPROFILE%\.cargo\bin & goto :fail)

echo [2/5] frontend deps...
if not exist node_modules call npm install --no-audit --no-fund || goto :fail

echo [3/5] ffmpeg sidecar (externalBin, auto-download if missing)...
if exist "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" (
  echo        sidecar exists, skip
) else (
  call :ensure_ffmpeg
  if errorlevel 1 goto :fail_ffmpeg
)

echo [4/5] Tauri build (release + NSIS exe + WiX msi^)...
call npm run tauri build || goto :fail

echo [5/5] collect artifacts...
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
  echo [FAIL] no installer artifacts found
  goto :fail
)
echo.
echo DONE. Installers at %CD%\%OUTDIR%\
dir /b %OUTDIR%

if not "%1"=="--nopause" pause
exit /b 0

:ensure_ffmpeg
if not exist src-tauri\binaries mkdir src-tauri\binaries
echo        downloading gyan.dev ffmpeg-release-essentials (~110MB, with retry^)...
curl -L --retry 3 --retry-delay 2 -C - -o "%TEMP%\ffmpeg-release-essentials.zip" "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
if errorlevel 1 exit /b 1
if exist "%TEMP%\ffmpeg-essentials-extract" rmdir /s /q "%TEMP%\ffmpeg-essentials-extract"
powershell -NoProfile -Command "Expand-Archive -Force '%TEMP%\ffmpeg-release-essentials.zip' '%TEMP%\ffmpeg-essentials-extract'"
if errorlevel 1 exit /b 1
set FFMPEG_FOUND=0
for /r "%TEMP%\ffmpeg-essentials-extract" %%F in (ffmpeg.exe) do (
  copy /y "%%~fF" "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" >nul
  set FFMPEG_FOUND=1
)
if not "%FFMPEG_FOUND%"=="1" (
  echo [FAIL] ffmpeg.exe not found inside extracted archive
  exit /b 1
)
echo        sidecar ready: src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe
exit /b 0

:fail
echo.
echo [BUILD FAILED]
if not "%1"=="--nopause" pause
exit /b 1

:fail_ffmpeg
echo.
echo [FAIL] ffmpeg sidecar download/extract failed. Manual steps:
echo        download https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip
echo        extract bin\ffmpeg.exe, rename to src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe
if not "%1"=="--nopause" pause
exit /b 1

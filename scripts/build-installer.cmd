@echo off
REM ============================================================
REM  Smart Photo Windows installer build script
REM  Output: NSIS exe (currentUser, no admin) + WiX msi
REM  Artifacts: installer-output\
REM  Usage: scripts\build-installer.cmd [--nopause]
REM  Notes: first run downloads NSIS/WiX toolchain to %LOCALAPPDATA%\tauri
REM ============================================================
setlocal enabledelayedexpansion
set PATH=%USERPROFILE%\.local\nodejs;%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0.."

echo [1/4] toolchain check...
where node >nul 2>nul || (echo [FAIL] node not found, expected %USERPROFILE%\.local\nodejs & goto :fail)
where cargo >nul 2>nul || (echo [FAIL] cargo not found, expected %USERPROFILE%\.cargo\bin & goto :fail)

echo [2/4] frontend deps...
if not exist node_modules call npm install --no-audit --no-fund || goto :fail

echo [3/4] Tauri build (release + NSIS exe + WiX msi^)...
call npm run tauri build || goto :fail

echo [4/4] collect artifacts...
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

:fail
echo.
echo [BUILD FAILED]
if not "%1"=="--nopause" pause
exit /b 1

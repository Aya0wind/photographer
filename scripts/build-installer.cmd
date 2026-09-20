@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion
REM ============================================================
REM  Smart Photo Windows 安装包构建脚本
REM  产物: NSIS exe (currentUser 免管理员) + WiX msi 双格式
REM  输出: installer-output\ 目录
REM  用法: scripts\build-installer.cmd [--nopause]
REM  说明: 首次打包会自动下载 NSIS/WiX 工具链到 %LOCALAPPDATA%\tauri
REM        ffmpeg 侧车缺失时自动下载（gyan.dev release-essentials）
REM ============================================================
set PATH=%USERPROFILE%\.local\nodejs;%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0.."

echo [1/5] 检查工具链...
where node >nul 2>nul || (echo [失败] 未找到 node（预期 %USERPROFILE%\.local\nodejs）& goto :fail)
where cargo >nul 2>nul || (echo [失败] 未找到 cargo（预期 %USERPROFILE%\.cargo\bin）& goto :fail)

echo [2/5] 前端依赖...
if not exist node_modules (
  call npm install --no-audit --no-fund || goto :fail
) else (
  echo        node_modules 已存在，跳过
)

echo [3/5] ffmpeg 侧车（externalBin，缺失自动下载，幂等）...
if exist "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" (
  echo        侧车已存在，跳过
) else (
  if not exist src-tauri\binaries mkdir src-tauri\binaries
  set FFMPEG_ZIP=%TEMP%\ffmpeg-release-essentials.zip
  set FFMPEG_EXTRACT=%TEMP%\ffmpeg-essentials-extract
  echo        下载 gyan.dev ffmpeg-release-essentials（约 110MB，含重试）...
  curl -L --retry 3 --retry-delay 2 -C - -o "%FFMPEG_ZIP%" "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip" || goto :fail_ffmpeg
  if exist "%FFMPEG_EXTRACT%" rmdir /s /q "%FFMPEG_EXTRACT%"
  powershell -NoProfile -Command "Expand-Archive -Force '%TEMP%\ffmpeg-release-essentials.zip' '%TEMP%\ffmpeg-essentials-extract'" || goto :fail_ffmpeg
  set FFMPEG_FOUND=0
  for /r "%FFMPEG_EXTRACT%" %%F in (ffmpeg.exe) do (
    copy /y "%%~fF" "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" >nul && set FFMPEG_FOUND=1
  )
  if not %FFMPEG_FOUND%==1 goto :fail_ffmpeg
  echo        侧车就位: src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe
)

echo [4/5] Tauri 构建（release 编译 + NSIS exe + WiX msi）...
call npm run tauri build || goto :fail

echo [5/5] 收集产物...
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

:fail_ffmpeg
echo.
echo [失败] ffmpeg 侧车下载/解压失败——可手动下载
echo        https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip
echo        解压出 bin\ffmpeg.exe 改名放到 src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe
if not "%1"=="--nopause" pause
exit /b 1

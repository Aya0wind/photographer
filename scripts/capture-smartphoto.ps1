# 真实 Tauri 窗口截图（视觉验收备份手段；主手段是 WebView2 CDP 远程调试）
# 用法: powershell -ExecutionPolicy Bypass -File scripts/capture-smartphoto.ps1 <输出.png> [smart-photo]
param(
    [Parameter(Mandatory = $true)][string]$OutPath,
    [string]$ProcessName = "smart-photo"
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Cap {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

[Win32Cap]::SetProcessDPIAware() | Out-Null

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1
if (-not $proc) {
    Write-Error "未找到窗口进程 '$ProcessName'（dev 构建进程名为 smart-photo）"
    exit 1
}

[Win32Cap]::SetForegroundWindow($proc.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 250

$rect = New-Object Win32Cap+RECT
[Win32Cap]::GetWindowRect($proc.MainWindowHandle, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { Write-Error "窗口尺寸无效 ${w}x${h}"; exit 1 }

$bounds = New-Object System.Drawing.Rectangle($rect.Left, $rect.Top, $w, $h)
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
$bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$gfx.Dispose(); $bmp.Dispose()
Write-Output "saved: $OutPath (${w}x${h})"

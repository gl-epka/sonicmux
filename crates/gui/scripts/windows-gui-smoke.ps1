param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [Parameter(Mandatory = $true)]
    [string]$Screenshot,

    [Parameter(Mandatory = $true)]
    [string]$Report
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class SonicMuxWindow {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetWindowRect(IntPtr handle, out Rect rect);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool MoveWindow(
        IntPtr handle,
        int x,
        int y,
        int width,
        int height,
        bool repaint
    );

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr handle);
}
"@

$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$screenshotDirectory = Split-Path -Parent $Screenshot
$reportDirectory = Split-Path -Parent $Report
New-Item -ItemType Directory -Force -Path $screenshotDirectory, $reportDirectory | Out-Null

$process = Start-Process -FilePath $resolvedExecutable -PassThru

try {
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 250
        $process.Refresh()

        if ($process.HasExited) {
            throw "SonicMux exited before creating a window (exit code $($process.ExitCode))."
        }
    } while (($process.MainWindowHandle -eq [IntPtr]::Zero) -and ([DateTime]::UtcNow -lt $deadline))

    if ($process.MainWindowHandle -eq [IntPtr]::Zero) {
        throw "SonicMux did not create a native window within 45 seconds."
    }

    if ($process.MainWindowTitle -ne "SonicMux") {
        throw "Unexpected window title: '$($process.MainWindowTitle)'."
    }

    $targetWidth = 760
    $targetHeight = 560
    if (-not [SonicMuxWindow]::MoveWindow(
        $process.MainWindowHandle,
        0,
        0,
        $targetWidth,
        $targetHeight,
        $true
    )) {
        throw "Could not resize the SonicMux window."
    }

    [SonicMuxWindow]::SetForegroundWindow($process.MainWindowHandle) | Out-Null
    Start-Sleep -Seconds 2

    $rect = New-Object SonicMuxWindow+Rect
    if (-not [SonicMuxWindow]::GetWindowRect($process.MainWindowHandle, [ref]$rect)) {
        throw "Could not inspect the SonicMux window bounds."
    }

    $actualWidth = $rect.Right - $rect.Left
    $actualHeight = $rect.Bottom - $rect.Top
    if (($actualWidth -lt $targetWidth) -or ($actualHeight -lt $targetHeight)) {
        throw "Window bounds $($actualWidth)x$($actualHeight) are smaller than the 760x560 contract."
    }

    $bitmap = New-Object System.Drawing.Bitmap($actualWidth, $actualHeight)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
        $bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }

    $executableHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolvedExecutable).Hash.ToLowerInvariant()
    $screenshotHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Screenshot).Hash.ToLowerInvariant()
    [ordered]@{
        result = "passed"
        executable = Split-Path -Leaf $resolvedExecutable
        executable_sha256 = $executableHash
        process_id = $process.Id
        window_title = $process.MainWindowTitle
        window_width = $actualWidth
        window_height = $actualHeight
        screenshot = Split-Path -Leaf $Screenshot
        screenshot_sha256 = $screenshotHash
        checked_at_utc = [DateTime]::UtcNow.ToString("o")
    } | ConvertTo-Json | Set-Content -LiteralPath $Report -Encoding utf8

    Write-Host "SonicMux native window passed: $($actualWidth)x$($actualHeight)."
}
finally {
    if (-not $process.HasExited) {
        $process.CloseMainWindow() | Out-Null
        if (-not $process.WaitForExit(5000)) {
            Stop-Process -Id $process.Id -Force
        }
    }
}

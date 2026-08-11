param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [Parameter(Mandatory = $true)]
    [string]$Screenshot,

    [Parameter(Mandatory = $true)]
    [string]$NativeScreenshot,

    [Parameter(Mandatory = $true)]
    [string]$Diagnostics,

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

function Invoke-CdpCommand {
    param(
        [Parameter(Mandatory = $true)]
        [System.Net.WebSockets.ClientWebSocket]$Socket,

        [Parameter(Mandatory = $true)]
        [int]$Id,

        [Parameter(Mandatory = $true)]
        [string]$Method,

        [hashtable]$Parameters = @{}
    )

    $message = [ordered]@{
        id = $Id
        method = $Method
        params = $Parameters
    } | ConvertTo-Json -Compress -Depth 20
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($message)
    $segment = [ArraySegment[byte]]::new($bytes)
    $Socket.SendAsync(
        $segment,
        [System.Net.WebSockets.WebSocketMessageType]::Text,
        $true,
        [System.Threading.CancellationToken]::None
    ).GetAwaiter().GetResult()

    do {
        $stream = [System.IO.MemoryStream]::new()
        try {
            do {
                $buffer = [byte[]]::new(65536)
                $receiveSegment = [ArraySegment[byte]]::new($buffer)
                $receiveResult = $Socket.ReceiveAsync(
                    $receiveSegment,
                    [System.Threading.CancellationToken]::None
                ).GetAwaiter().GetResult()

                if ($receiveResult.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
                    throw "WebView2 closed the DevTools connection unexpectedly."
                }

                $stream.Write($buffer, 0, $receiveResult.Count)
            } while (-not $receiveResult.EndOfMessage)

            $payload = [System.Text.Encoding]::UTF8.GetString($stream.ToArray()) | ConvertFrom-Json
            $payloadIdProperty = $payload.PSObject.Properties["id"]
        }
        finally {
            $stream.Dispose()
        }
    } while (($null -eq $payloadIdProperty) -or ($payloadIdProperty.Value -ne $Id))

    $errorProperty = $payload.PSObject.Properties["error"]
    if ($null -ne $errorProperty) {
        throw "DevTools command '$Method' failed: $($errorProperty.Value.message)"
    }

    return $payload.result
}

$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$screenshotDirectory = Split-Path -Parent $Screenshot
$nativeScreenshotDirectory = Split-Path -Parent $NativeScreenshot
$diagnosticsDirectory = Split-Path -Parent $Diagnostics
$reportDirectory = Split-Path -Parent $Report
New-Item -ItemType Directory -Force -Path $screenshotDirectory, $nativeScreenshotDirectory, $diagnosticsDirectory, $reportDirectory | Out-Null

$debuggingPort = 9227
$browserArguments = "--remote-debugging-port=$debuggingPort --remote-allow-origins=*"

$process = Start-Process -FilePath $resolvedExecutable -PassThru
if ($null -eq $process) {
    throw "Windows did not start the SonicMux process."
}

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
        $bitmap.Save($NativeScreenshot, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }

    $webViewProcesses = @(
        Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" |
            Select-Object ProcessId, ParentProcessId, CommandLine
    )
    [ordered]@{
        requested_browser_arguments = $browserArguments
        debugging_port = $debuggingPort
        processes = $webViewProcesses
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Diagnostics -Encoding utf8

    if ($webViewProcesses.Count -eq 0) {
        Write-Warning "No msedgewebview2.exe processes were visible to the smoke test."
    }
    else {
        Write-Host "Observed WebView2 processes:"
        $webViewProcesses | ForEach-Object { Write-Host $_.CommandLine }
    }

    $debugDeadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        try {
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$debuggingPort/json/list" -TimeoutSec 2
            $pageTarget = $targets | Where-Object { $_.type -eq "page" } | Select-Object -First 1
        }
        catch {
            $pageTarget = $null
        }

        if ($null -eq $pageTarget) {
            Start-Sleep -Milliseconds 250
        }
    } while (($null -eq $pageTarget) -and ([DateTime]::UtcNow -lt $debugDeadline))

    if ($null -eq $pageTarget) {
        throw "WebView2 did not expose a debuggable page within 30 seconds."
    }

    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    try {
        $socket.ConnectAsync(
            [Uri]$pageTarget.webSocketDebuggerUrl,
            [System.Threading.CancellationToken]::None
        ).GetAwaiter().GetResult()

        $commandId = 1
        Invoke-CdpCommand -Socket $socket -Id $commandId -Method "Page.enable" | Out-Null
        $commandId++
        Invoke-CdpCommand -Socket $socket -Id $commandId -Method "Runtime.enable" | Out-Null

        $domDeadline = [DateTime]::UtcNow.AddSeconds(30)
        do {
            $commandId++
            $evaluation = Invoke-CdpCommand -Socket $socket -Id $commandId -Method "Runtime.evaluate" -Parameters @{
                expression = "JSON.stringify({readyState: document.readyState, text: document.body?.innerText ?? '', width: window.innerWidth, height: window.innerHeight})"
                returnByValue = $true
            }
            $webViewState = $evaluation.result.value | ConvertFrom-Json
            $domReady = (
                ($webViewState.readyState -eq "complete") -and
                ($webViewState.text -like "*SonicMux*") -and
                ($webViewState.text -like "*FFmpeg*")
            )

            if (-not $domReady) {
                Start-Sleep -Milliseconds 250
            }
        } while ((-not $domReady) -and ([DateTime]::UtcNow -lt $domDeadline))

        if (-not $domReady) {
            throw "WebView2 DOM did not render the expected SonicMux and FFmpeg content."
        }

        $commandId++
        $capture = Invoke-CdpCommand -Socket $socket -Id $commandId -Method "Page.captureScreenshot" -Parameters @{
            format = "png"
            fromSurface = $true
            captureBeyondViewport = $false
        }
        [System.IO.File]::WriteAllBytes($Screenshot, [Convert]::FromBase64String($capture.data))
    }
    finally {
        $socket.Dispose()
    }

    $executableHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolvedExecutable).Hash.ToLowerInvariant()
    $screenshotHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Screenshot).Hash.ToLowerInvariant()
    $nativeScreenshotHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $NativeScreenshot).Hash.ToLowerInvariant()
    [ordered]@{
        result = "passed"
        executable = Split-Path -Leaf $resolvedExecutable
        executable_sha256 = $executableHash
        process_id = $process.Id
        window_title = $process.MainWindowTitle
        window_width = $actualWidth
        window_height = $actualHeight
        webview_ready_state = $webViewState.readyState
        webview_width = $webViewState.width
        webview_height = $webViewState.height
        webview_text_characters = $webViewState.text.Length
        expected_content = @("SonicMux", "FFmpeg")
        screenshot = Split-Path -Leaf $Screenshot
        screenshot_sha256 = $screenshotHash
        native_screenshot = Split-Path -Leaf $NativeScreenshot
        native_screenshot_sha256 = $nativeScreenshotHash
        diagnostics = Split-Path -Leaf $Diagnostics
        checked_at_utc = [DateTime]::UtcNow.ToString("o")
    } | ConvertTo-Json | Set-Content -LiteralPath $Report -Encoding utf8

    Write-Host "SonicMux native window and WebView2 DOM passed: $($actualWidth)x$($actualHeight)."
}
finally {
    if (-not $process.HasExited) {
        $process.CloseMainWindow() | Out-Null
        if (-not $process.WaitForExit(5000)) {
            Stop-Process -Id $process.Id -Force
        }
    }
}

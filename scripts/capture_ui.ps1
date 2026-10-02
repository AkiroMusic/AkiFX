# Capture the AkiFX standalone window to a PNG.
# Usage:
#   pwsh scripts/capture_ui.ps1 -ExePath target/release/akifx.exe -OutPng shot.png `
#       [-Clicks "36,123;150,293"] [-Seconds 3]
# Clicks are editor-logical coordinates (1280x800 design space); they are
# scaled by the window's actual client size. Used to enable modules / select
# the Spectral Compressor before the shot. The window is made topmost and the
# screen region is copied — GL windows do not blit through PrintWindow.

param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$OutPng,
    [string]$Clicks = "",
    [string]$DragFrom = "",
    [string]$DragTo = "",
    [int]$Seconds = 3
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Capture {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int X, int Y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct POINT { public int X, Y; }
}
"@

[Win32Capture]::SetProcessDPIAware() | Out-Null

$proc = Start-Process -FilePath $ExePath -PassThru
try {
    # Wait for the plugin window
    $hwnd = [IntPtr]::Zero
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 250
        $proc.Refresh()
        if ($proc.MainWindowHandle -ne 0) { $hwnd = $proc.MainWindowHandle; break }
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw "no window handle found" }
    Start-Sleep -Seconds 2

    # Topmost so the screen copy only contains this window's pixels.
    $HWND_TOPMOST = [IntPtr](-1)
    [Win32Capture]::SetWindowPos($hwnd, $HWND_TOPMOST, 0, 0, 0, 0, 0x0003) | Out-Null # NOMOVE|NOSIZE
    Start-Sleep -Milliseconds 600

    $client = New-Object Win32Capture+RECT
    $win = New-Object Win32Capture+RECT
    [Win32Capture]::GetClientRect($hwnd, [ref]$client) | Out-Null
    [Win32Capture]::GetWindowRect($hwnd, [ref]$win) | Out-Null
    $clientW = $client.Right - $client.Left
    $clientH = $client.Bottom - $client.Top
    $scale = $clientW / 1280.0
    # Frame offset between window rect and client rect
    $offX = ($win.Right - $win.Left - $clientW) / 2
    $offY = ($win.Bottom - $win.Top - $clientH) - $offX # bottom border == top border
    $originX = $win.Left + $offX
    $originY = $win.Top + $offY

    function Invoke-Click([double]$lx, [double]$ly) {
        # Real input event at the physical position of the logical point —
        # the window is topmost, so the click lands on it and activates it.
        $px = [int]($originX + $lx * $scale)
        $py = [int]($originY + $ly * $scale)
        [Win32Capture]::SetCursorPos($px, $py) | Out-Null
        Start-Sleep -Milliseconds 200
        [Win32Capture]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero) # LEFTDOWN
        Start-Sleep -Milliseconds 90
        [Win32Capture]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) # LEFTUP
        Start-Sleep -Milliseconds 500
        Write-Output ("clicked physical ({0},{1})" -f $px, $py)
    }

    if ($Clicks -ne "") {
        foreach ($c in $Clicks -split ";") {
            $parts = $c -split ","
            Invoke-Click ([double]$parts[0]) ([double]$parts[1])
        }
    }

    Start-Sleep -Seconds $Seconds

    # Mid-drag capture: press at DragFrom, move to DragTo with the button
    # held, capture while held, then release.
    if ($DragFrom -ne "" -and $DragTo -ne "") {
        $f = $DragFrom -split ","
        $t = $DragTo -split ","
        $fx = [int]($originX + [double]$f[0] * $scale)
        $fy = [int]($originY + [double]$f[1] * $scale)
        $tx = [int]($originX + [double]$t[0] * $scale)
        $ty = [int]($originY + [double]$t[1] * $scale)
        [Win32Capture]::SetCursorPos($fx, $fy) | Out-Null
        Start-Sleep -Milliseconds 400
        [Win32Capture]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero) # down (grip)
        Start-Sleep -Milliseconds 350
        # Jiggle first — egui needs a few px of pressed movement to decide on
        # a drag, then walk to the target.
        [Win32Capture]::SetCursorPos($fx + 3, $fy + 3) | Out-Null
        Start-Sleep -Milliseconds 200
        foreach ($step in 1..8) {
            $mx = [int]($fx + 3 + ($tx - $fx) * $step / 8)
            $my = [int]($fy + 3 + ($ty - $fy) * $step / 8)
            [Win32Capture]::SetCursorPos($mx, $my) | Out-Null
            Start-Sleep -Milliseconds 110
        }
        Start-Sleep -Milliseconds 600
    }

    # Copy the whole window rect from the composited screen (clicks use the
    # client origin above; the capture includes the OS frame, which the
    # reviewer is told to ignore).
    $winW = $win.Right - $win.Left
    $winH = $win.Bottom - $win.Top
    $bmp = New-Object System.Drawing.Bitmap($winW, $winH)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($win.Left, $win.Top, 0, 0, (New-Object System.Drawing.Size($winW, $winH)))
    $g.Dispose()

    if ($DragFrom -ne "" -and $DragTo -ne "") {
        [Win32Capture]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) # up
        Start-Sleep -Milliseconds 300
    }

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutPng) | Out-Null
    $bmp.Save($OutPng, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "saved: $OutPng ($winW x $winH window)"
}
finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}

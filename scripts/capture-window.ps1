# 抓取学习中枢主窗口的截图（Windows）。
#
# 用 PrintWindow + PW_RENDERFULLCONTENT 直接取窗口内容，不受遮挡影响；
# 失败（全黑）时退回「置顶 + 屏幕区域拷贝」。
#
# 用法：powershell -ExecutionPolicy Bypass -File scripts/capture-window.ps1 [-Out out.png] [-ProcessName learning-hub]

param(
  [string]$Out = "screenshot.png",
  [string]$ProcessName = "learning-hub",
  [switch]$ScreenFallback
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class HubWin {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
}
"@

function Get-AppWindows([int[]]$pids) {
  $found = New-Object System.Collections.ArrayList
  $cb = [HubWin+EnumProc]{
    param($hwnd, $lparam)
    $wpid = 0
    [HubWin]::GetWindowThreadProcessId($hwnd, [ref]$wpid) | Out-Null
    if ($pids -contains [int]$wpid) {
      $sb = New-Object System.Text.StringBuilder 512
      [HubWin]::GetWindowTextW($hwnd, $sb, 512) | Out-Null
      $title = $sb.ToString()
      if ($title.Length -gt 0) {
        $r = New-Object HubWin+RECT
        [HubWin]::GetWindowRect($hwnd, [ref]$r) | Out-Null
        $vis = [HubWin]::IsWindowVisible($hwnd)
        [void]$found.Add([pscustomobject]@{
          Hwnd = $hwnd; Title = $title; Visible = $vis
          W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
          Left = $r.Left; Top = $r.Top
        })
      }
    }
    return $true
  }
  [HubWin]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  return $found
}

$procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if (-not $procs) { Write-Error "process '$ProcessName' not running"; exit 1 }

$pids = @($procs | ForEach-Object { [int]$_.Id })
Write-Output ("pids: " + ($pids -join ","))

$wins = Get-AppWindows $pids
foreach ($w in $wins) {
  Write-Output ("window: '{0}' {1}x{2} visible={3}" -f $w.Title, $w.W, $w.H, $w.Visible)
}
if ($wins.Count -eq 0) { Write-Error "no titled window found"; exit 1 }

# 取最大的那个（主窗口通常最大）
$win = $wins | Sort-Object -Property @{Expression = { $_.W * $_.H }} -Descending | Select-Object -First 1
if ($win.W -le 0 -or $win.H -le 0) { Write-Error "bad window rect"; exit 1 }

$full = [System.IO.Path]::GetFullPath($Out)
$dir = [System.IO.Path]::GetDirectoryName($full)
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

function Save-Bitmap($bmp, $path) {
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
}

# ---- 首选：PrintWindow（能抓被遮挡的窗口）----
$bmp = New-Object System.Drawing.Bitmap($win.W, $win.H)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [HubWin]::PrintWindow($win.Hwnd, $hdc, 2)   # 2 = PW_RENDERFULLCONTENT
$g.ReleaseHdc($hdc)
$g.Dispose()

# 判断是否全黑（PrintWindow 对某些合成窗口会失败）
$nonBlack = 0
for ($y = 0; $y -lt $win.H; $y += 24) {
  for ($x = 0; $x -lt $win.W; $x += 24) {
    $c = $bmp.GetPixel($x, $y)
    if ($c.R -gt 12 -or $c.G -gt 12 -or $c.B -gt 12) { $nonBlack++ }
  }
}

if ($ok -and $nonBlack -gt 20) {
  Save-Bitmap $bmp $full
  $bmp.Dispose()
  Write-Output "saved (PrintWindow): $full ($($win.W) x $($win.H))"
  exit 0
}
$bmp.Dispose()
Write-Output "PrintWindow produced a blank frame (nonBlack=$nonBlack), falling back to screen copy"

# ---- 退回：置顶 + 屏幕区域拷贝 ----
[HubWin]::ShowWindow($win.Hwnd, 9) | Out-Null     # SW_RESTORE
[HubWin]::SetForegroundWindow($win.Hwnd) | Out-Null
Start-Sleep -Milliseconds 900

$bmp2 = New-Object System.Drawing.Bitmap($win.W, $win.H)
$g2 = [System.Drawing.Graphics]::FromImage($bmp2)
$g2.CopyFromScreen($win.Left, $win.Top, 0, 0, $bmp2.Size)
Save-Bitmap $bmp2 $full
$g2.Dispose()
$bmp2.Dispose()
Write-Output "saved (screen copy): $full ($($win.W) x $($win.H))"

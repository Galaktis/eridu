# Runs the release build on a repository and screenshots its window at given times (seconds).
# Usage: tools\screenshot.ps1 -Repo path\to\repo -At 15,45 -Out $env:TEMP [-Extra "--start","50%"]
# Prints the saved PNG paths, then any WARN/ERROR/panic lines from the app's logs.
param(
    [Parameter(Mandatory)] [string]$Repo,
    [int[]]$At = @(10, 30),
    [string]$Out = $env:TEMP,
    [string]$Prefix = "eridu",
    # Extra command-line options for eridu.
    [string[]]$Extra = @()
)
$exe = Join-Path $PSScriptRoot "..\target\release\eridu.exe"
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public class EriduWindow {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  public struct RECT { public int L, T, R, B; }
  // The process's MainWindowHandle is its console, so find the game window by title instead.
  public static IntPtr Find(string title) { IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
      // The app's window is titled "Eridu, em dash, repo name"; the dash is written as a char
      // code so this file's encoding doesn't matter (Windows PowerShell reads it as ANSI).
      var t = sb.ToString();
      if (IsWindowVisible(h) && (t == title || t.StartsWith(title + " " + (char)0x2014 + " "))) { found = h; return false; } return true; }, IntPtr.Zero);
    return found; }
}
'@
# Without this, Windows display scaling shrinks the window rectangle and crops the capture.
[EriduWindow]::SetProcessDPIAware() | Out-Null
$log = Join-Path $Out "$Prefix.log"
$p = Start-Process $exe -ArgumentList (($Extra + $Repo) | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -PassThru -RedirectStandardError $log -RedirectStandardOutput "$log.out"
$prev = 0
foreach ($t in $At) {
    Start-Sleep -Seconds ($t - $prev); $prev = $t
    $h = [EriduWindow]::Find("Eridu")
    if ($h -eq [IntPtr]::Zero) { "no window at ${t}s"; continue }
    $r = New-Object EriduWindow+RECT; [EriduWindow]::GetWindowRect($h, [ref]$r) | Out-Null
    $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
    # PrintWindow with PW_RENDERFULLCONTENT (2) captures the window itself, even when other
    # windows cover it, instead of whatever happens to be on screen.
    $g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
    [EriduWindow]::PrintWindow($h, $hdc, 2) | Out-Null
    $g.ReleaseHdc($hdc); $g.Dispose()
    $png = Join-Path $Out "${Prefix}_$t.png"; $bmp.Save($png); $png
}
Stop-Process -Id $p.Id -Confirm:$false -ErrorAction SilentlyContinue
Select-String -Path $log, "$log.out" -Pattern "WARN|ERROR|panick|authors" | ForEach-Object { $_.Line -replace '\x1b\[[0-9;]*m', '' }

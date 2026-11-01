# Regenerate the README screenshots docs/popup.png and docs/settings.png.
# Run from repo root, and leave the keyboard and mouse alone while it runs:
#   powershell -ExecutionPolicy Bypass -File scripts/screenshots.ps1
# Builds the debug app twice (dark, then light), stops any running popslate, and
# needs Microsoft Edge for the final images. Captures at 1x, so run it at 100% scaling.
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\.."
$exe = "$root\src-tauri\target\debug\popslate.exe"
$conf = "$root\src-tauri\tauri.conf.json"
$raw = "$env:TEMP\popslate-shots"
New-Item -ItemType Directory -Force $raw | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
}
"@
[void][W]::SetProcessDPIAware()

function Grab($x, $y, $w, $h, $file) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  [System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($x, $y, 0, 0, $bmp.Size); $bmp.Save($file); $bmp.Dispose()
}
# Tapping an unassigned key makes this the "last input" process, so it may take the foreground.
function Unlock { [W]::keybd_event(0xE8, 0, 0, [UIntPtr]::Zero); [W]::keybd_event(0xE8, 0, 2, [UIntPtr]::Zero) }

function Capture($Theme) {
  Get-Process popslate -ErrorAction SilentlyContinue | Stop-Process -Confirm:$false
  $app = Start-Process $exe -PassThru -WindowStyle Hidden; Start-Sleep 3
  $popup = [W]::FindWindow("Tauri Window", "Popslate")
  if ($popup -eq [IntPtr]::Zero) { Stop-Process $app; throw "popup window not found" }

  # Popup: a borderless sample document with one sentence selected, then Alt+Q.
  $dark = $Theme -eq "dark"
  $bg = if ($dark) { [System.Drawing.Color]::FromArgb(31, 31, 31) } else { [System.Drawing.Color]::White }
  $fg = if ($dark) { [System.Drawing.Color]::FromArgb(230, 230, 230) } else { [System.Drawing.Color]::FromArgb(30, 30, 30) }
  $form = New-Object System.Windows.Forms.Form
  $form.FormBorderStyle = 'None'; $form.TopMost = $true; $form.StartPosition = 'Manual'
  $form.Location = New-Object System.Drawing.Point(200, 150); $form.Size = New-Object System.Drawing.Size(400, 190)
  $form.BackColor = $bg; $form.Padding = New-Object System.Windows.Forms.Padding(24, 22, 24, 22)
  $tb = New-Object System.Windows.Forms.RichTextBox
  $tb.Dock = 'Fill'; $tb.BorderStyle = 'None'; $tb.BackColor = $bg; $tb.ForeColor = $fg; $tb.ScrollBars = 'None'; $tb.HideSelection = $false
  $tb.Font = New-Object System.Drawing.Font 'Segoe UI', 12
  $sentence = "Small habits shape big results."
  $tb.Text = "$sentence`n`nA few minutes of practice every day, repeated for months, quietly turns into real skill."
  $form.Controls.Add($tb)
  $script:step = 0
  $timer = New-Object System.Windows.Forms.Timer; $timer.Interval = 800
  $timer.Add_Tick({
    $script:step++
    if ($script:step -eq 1) {
      Unlock; [void][W]::SetForegroundWindow($form.Handle); $tb.Focus()
      $start = $tb.Text.IndexOf($sentence); $tb.Select($start, $sentence.Length)
      # Cursor where a mouse selection would end: the popup opens next to it.
      $pt = $tb.PointToScreen($tb.GetPositionFromCharIndex($start + 3)); [void][W]::SetCursorPos($pt.X, $pt.Y + 26)
      [System.Windows.Forms.SendKeys]::SendWait("%q")
    }
    if ($script:step -eq 3) {
      if ([W]::IsWindowVisible($popup)) { Grab $form.Left $form.Top $form.Width $form.Height "$raw\popup-$Theme.png" }
      else { $script:failed = "popup not visible" }
      $timer.Stop(); $form.Close()
    }
  })
  $form.Add_Shown({ $timer.Start() })
  [System.Windows.Forms.Application]::Run($form)
  if ($script:failed) { Stop-Process $app; throw $script:failed }

  # Settings: a second launch opens it. A plain backdrop hides the desktop behind its corners.
  Start-Process $exe; Start-Sleep 2.5
  $set = [W]::FindWindow("Tauri Window", "Popslate Settings")
  if ($set -eq [IntPtr]::Zero) { Stop-Process $app; throw "settings window not found" }
  $r = New-Object W+RECT; [void][W]::DwmGetWindowAttribute($set, 9, [ref]$r, 16)   # visible frame only
  $m = 24
  $bd = New-Object System.Windows.Forms.Form
  $bd.FormBorderStyle = 'None'; $bd.StartPosition = 'Manual'; $bd.ShowInTaskbar = $false
  $bd.BackColor = if ($dark) { [System.Drawing.Color]::FromArgb(32, 32, 32) } else { [System.Drawing.Color]::FromArgb(238, 238, 238) }
  $bd.Location = New-Object System.Drawing.Point(($r.L - $m), ($r.T - $m)); $bd.Size = New-Object System.Drawing.Size(($r.R - $r.L + 2 * $m), ($r.B - $r.T + 2 * $m))
  $bd.Show(); [System.Windows.Forms.Application]::DoEvents()
  Unlock; [void][W]::SetForegroundWindow($bd.Handle); Start-Sleep -Milliseconds 200
  Unlock; [void][W]::SetForegroundWindow($set); Start-Sleep -Milliseconds 600; [System.Windows.Forms.Application]::DoEvents()
  Grab $r.L $r.T ($r.R - $r.L) ($r.B - $r.T) "$raw\settings-$Theme.png"
  $bd.Close()
  Stop-Process $app -Confirm:$false
}

function Build { cargo build --manifest-path "$root\src-tauri\Cargo.toml"; if ($LASTEXITCODE) { throw "cargo build failed" } }

# Dark: force the theme in the config for one build. Light last, so the debug build ends up normal.
$original = [IO.File]::ReadAllText($conf)
try {
  [IO.File]::WriteAllText($conf, $original.Replace('"create": false', '"create": false, "theme": "Dark"'))
  Build; Capture "dark"
} finally { [IO.File]::WriteAllText($conf, $original) }
Build; Capture "light"

# Compose: light and dark side by side on a gradient, 880 px wide (the README content width),
# so GitHub shows the image 1:1.
$ErrorActionPreference = "Continue"   # Edge reports progress on stderr
$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
$profile = "$raw\edge-profile"
function Compose($name, $tileW, $tileH) {
  $h = $tileH + 2 * 44
  $html = @"
<!doctype html><html><head><meta charset="utf-8"><style>
  html, body { margin: 0; width: 880px; height: ${h}px; overflow: hidden; }
  body { display: flex; align-items: center; justify-content: center; gap: 24px;
    background: radial-gradient(circle at 15% 10%, #ffc49b 0%, transparent 45%),
                linear-gradient(135deg, #ff8a5c 0%, #ff5a36 50%, #c8385f 100%); }
  .tile { width: ${tileW}px; height: ${tileH}px; border-radius: 10px; overflow: hidden;
    box-shadow: 0 2px 6px rgba(0,0,0,.18), 0 18px 40px rgba(60,10,20,.35); }
  .tile img { display: block; }
</style></head><body>
  <div class="tile"><img src="$name-light.png"></div>
  <div class="tile"><img src="$name-dark.png"></div>
</body></html>
"@
  Set-Content "$raw\$name.html" $html -Encoding UTF8
  & $edge --headless=new --disable-gpu --hide-scrollbars --user-data-dir="$profile" `
    --window-size="880,$h" --screenshot="$root\docs\$name.png" "file:///$($raw.Replace('\','/'))/$name.html" 2>$null | Out-Null
  Start-Sleep 1
}
$s = [System.Drawing.Image]::FromFile("$raw\settings-light.png"); $sw = $s.Width; $sh = $s.Height; $s.Dispose()
Compose "popup" 400 190
Compose "settings" $sw $sh
Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
"saved docs\popup.png and docs\settings.png"

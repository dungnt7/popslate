# End-to-end check of hotkey -> selection -> translation, using a throwaway test window.
# Run from repo root after `cargo build` (in src-tauri):
#   powershell -ExecutionPolicy Bypass -File scripts/e2e-selection.ps1
# Stops any running popslate first. Restores your clipboard text at the end.
$ErrorActionPreference = "Stop"
$exe = "$PSScriptRoot\..\src-tauri\target\debug\popslate.exe"
$log = "$env:TEMP\popslate-e2e.log"

# One case per run of this same script with -Case (needs its own STA process for WinForms).
if ($args[0] -eq "-Case") {
    $Mode = $args[1]
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Fg {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
"@
    $form = New-Object System.Windows.Forms.Form
    $form.Text = "popslate e2e: $Mode"; $form.TopMost = $true
    if ($Mode -eq "fallback") {
        # No TextPattern: forces the Ctrl+C path. Answers Ctrl+C itself.
        $form.KeyPreview = $true
        $form.Add_KeyDown({ if ($_.Control -and $_.KeyCode -eq 'C') { [System.Windows.Forms.Clipboard]::SetText("The quick brown fox") } })
    } else {
        # RichTextBox supports TextPattern (UIA path); TextBox does not (Ctrl+C path).
        $tb = if ($Mode -eq "uia") { New-Object System.Windows.Forms.RichTextBox } else { New-Object System.Windows.Forms.TextBox }
        $tb.Multiline = $true; $tb.Dock = 'Fill'; $tb.Text = "The quick brown fox jumps over the lazy dog."
        $form.Controls.Add($tb)
    }
    $timer = New-Object System.Windows.Forms.Timer; $timer.Interval = 800; $script:step = 0
    $timer.Add_Tick({
        $script:step++
        if ($script:step -eq 1) {
            # Sending input makes this the "last input" process, which may take the foreground.
            [Fg]::keybd_event(0xE8, 0, 0, [UIntPtr]::Zero); [Fg]::keybd_event(0xE8, 0, 2, [UIntPtr]::Zero)
            [void][Fg]::SetForegroundWindow($form.Handle); $form.Activate()
            if ([Fg]::GetForegroundWindow() -ne $form.Handle) { Write-Output "TEST WINDOW NOT FOCUSED" }
            if ($tb) { $tb.Focus(); if ($Mode -eq "none") { $tb.Select(0, 0) } else { $tb.Select(4, 15) } }
            [System.Windows.Forms.SendKeys]::SendWait("%q")
        }
        if ($script:step -eq 4) { $form.Close() }
    })
    $form.Add_Shown({ $timer.Start() })
    [System.Windows.Forms.Application]::Run($form)
    exit
}

$userClip = Get-Clipboard -Raw
Get-Process popslate -ErrorAction SilentlyContinue | Stop-Process -Confirm:$false
Remove-Item $log -ErrorAction SilentlyContinue
$app = Start-Process $exe -RedirectStandardError $log -WindowStyle Hidden -PassThru
Start-Sleep 2

# case, expected log text
$cases = @(
    @("uia", "[selection] uia"),
    @("edit", "[selection] clipboard"),
    @("fallback", "[selection] clipboard"),
    @("none", "[hotkey] no selection")
)
$failed = 0
foreach ($c in $cases) {
    Set-Clipboard "E2E SENTINEL"
    $before = @(Get-Content $log).Count
    $focus = powershell -STA -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Case $c[0]
    Start-Sleep 1
    $out = ((Get-Content $log | Select-Object -Skip $before) + $focus) -join "`n"
    $ok = $out.Contains($c[1]) -and ((Get-Clipboard -Raw).Trim() -eq "E2E SENTINEL")
    if ($c[0] -ne "none") { $ok = $ok -and $out.Contains("nhanh") }   # "quick" in Vietnamese
    if (-not $ok) { $failed++ }
    "{0} {1}`n    {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" }), $c[0], $out.Replace("`n", "`n    "))
}
Stop-Process $app -Confirm:$false
if ($userClip) { Set-Clipboard $userClip }
if ($failed) { "$failed case(s) failed"; exit 1 } else { "all passed" }

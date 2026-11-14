# Run only inside an owned candidate lyrel pane. Uses the prepared, official
# libsixel 1.10.5 producer without changing the tools or user configuration.
param(
    [ValidateSet('Baseline', 'Scroll', 'Display', 'Clear', 'Refusal', 'Recovery', 'Discovery')]
    [string]$Scene = 'Baseline',
    [switch]$KeepVisible
)
$ErrorActionPreference = 'Stop'
$out = [Console]::OpenStandardOutput()
function Write-Vt([string]$text) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($text)
    $out.Write($bytes, 0, $bytes.Length)
    $out.Flush()
}
function Invoke-SixelProducer([string[]]$ExtraArgs) {
    # The producer is a real external process: it exits before the scene does,
    # so a later window reopen proves restoration without rerunning it.
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $producer = 'LD_LIBRARY_PATH=/home/dlivxpr/lyrel258-sixel-tools/root/usr/lib/x86_64-linux-gnu ' +
        '/home/dlivxpr/lyrel258-sixel-tools/root/usr/bin/img2sixel ' +
        ($ExtraArgs -join ' ')
    foreach ($arg in @('-d', 'Ubuntu-26.04', '--', 'bash', '-c', $producer)) {
        $start.ArgumentList.Add($arg)
    }
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $process = [Diagnostics.Process]::Start($start)
    $process.StandardOutput.BaseStream.CopyTo($out)
    $out.Flush()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw "img2sixel exited $($process.ExitCode)" }
    $process.Dispose()
}
$esc = [char]27
if ($Scene -eq 'Discovery') {
    # The Node scene owns the DA1/XTSMGRAPHICS probes; this script only provides
    # the real pane. It is not a producer and proves no pixels.
    Write-Vt "${esc}[2J${esc}[HSIXEL_275_DISCOVERY`r`n"
    node scripts/terminal-capabilities.mjs
    Write-Vt "`r`nSIXEL_275_DISCOVERY_DONE`r`n"
    return
}
Write-Vt "${esc}[2J${esc}[H${esc}[?80l${esc}[?8452lSIXEL_274_$Scene`r`n"
Write-Vt "Official img2sixel 1.10.5: static image then AFTER_SIXEL_274`r`n"
switch ($Scene) {
    'Scroll' { Write-Vt "${esc}[999;5H" }
    'Display' { Write-Vt "${esc}[?80h${esc}[6;5H" }
    default { Write-Vt "${esc}[6;5H" }
}
if ($Scene -eq 'Refusal') {
    # Rio rejects this raster before image admission. The following text must
    # remain at row 6, column 5, and existing images must not be replaced.
    Write-Vt "${esc}Pq`"1;1;65535;65535#0;2;100;0;0~${esc}\AFTER_SIXEL_274_REFUSED"
} else {
    Invoke-SixelProducer @('-w', '240', '-h', '160', '-p', '4', '-d', 'none',
        '/home/dlivxpr/lyrel258-sixel-tools/colors.ppm')
    Write-Vt 'AFTER_SIXEL_274'
    if ($Scene -eq 'Clear') { Write-Vt "${esc}[2J${esc}[HCLEARED_SIXEL_274" }
}
if ($Scene -eq 'Recovery') {
    # The image producer has exited. Evict its bytes with SGR resets only:
    # visible filler would wrap and scroll the image out of retained history,
    # testing normal image retirement instead of same-session restoration.
    # Reopening must recover the picture without rerunning the producer.
    Write-Vt "`r`nSIXEL_275_RECOVERY_TRIMMING`r`n"
    $batch = "${esc}[0m" * 1024
    # 2304 * 1024 * 4 bytes = 9 MiB, without moving the cursor or image.
    for ($index = 0; $index -lt 2304; $index++) {
        Write-Vt $batch
    }
    Write-Vt "SIXEL_275_RECOVERY_READY close and reopen this window, then press Enter here`r`n"
    [void](Read-Host 'Close/reopen the window, verify the image and AFTER_SIXEL_274, then press Enter')
    Write-Vt "SIXEL_275_RECOVERY_DONE`r`n"
}
if ($KeepVisible) { [void](Read-Host 'Capture the owned candidate pane; press Enter to finish') }
Write-Vt "${esc}[?80l${esc}[?8452l`r`nSIXEL_274_SCENE_DONE`r`n"

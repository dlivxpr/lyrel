# Run only inside an owned candidate lyrel pane. Uses the prepared, official
# libsixel 1.10.5 producer without changing the tools or user configuration.
param(
    [ValidateSet('Baseline', 'Scroll', 'Display', 'Clear', 'Refusal')]
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
$esc = [char]27
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
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    foreach ($arg in @('-d', 'Ubuntu-26.04', '--', 'bash', '-c',
        'LD_LIBRARY_PATH=/home/dlivxpr/lyrel258-sixel-tools/root/usr/lib/x86_64-linux-gnu /home/dlivxpr/lyrel258-sixel-tools/root/usr/bin/img2sixel -w 240 -h 160 -p 4 -d none /home/dlivxpr/lyrel258-sixel-tools/colors.ppm')) {
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
    Write-Vt 'AFTER_SIXEL_274'
    if ($Scene -eq 'Clear') { Write-Vt "${esc}[2J${esc}[HCLEARED_SIXEL_274" }
}
if ($KeepVisible) { [void](Read-Host 'Capture the owned candidate pane; press Enter to finish') }
Write-Vt "${esc}[?80l${esc}[?8452l`r`nSIXEL_274_SCENE_DONE`r`n"

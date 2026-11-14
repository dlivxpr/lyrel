# Run only inside an owned candidate lyrel pane. Produces static iTerm2 images
# through the recorded official imgcat script in its legacy single `File=` mode
# and a deterministic local PNG. It never downloads, resolves a remote path,
# edits user configuration, or starts a GUI; the caller owns the pane.
param(
    [ValidateSet('Baseline', 'Cells', 'Pixels', 'Percent', 'Stretch', 'Scroll', 'Clear', 'Refusal')]
    [string]$Scene = 'Baseline',
    # Official https://iterm2.com/utilities/imgcat, 8621 bytes, recorded here:
    [string]$Imgcat = '/mnt/c/Users/dlivx/AppData/Local/Temp/lyrel-spec258/imgcat-official.sh',
    [switch]$KeepVisible
)
$ErrorActionPreference = 'Stop'
$out = [Console]::OpenStandardOutput()
function Write-Vt([string]$text) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($text)
    $out.Write($bytes, 0, $bytes.Length)
    $out.Flush()
}

# `fixturePicture(3)` from scripts/graphics-fixture-utils.mjs: 48x24 RGBA, four
# quadrants and a white cross; first pixel is [255, 0, 33, 255].
$fixtureSha = '3D6A133DD26429D40B4ACB1C72784AFA65642A16E6B38C91A6E20320465F041E'
$pngBase64 = 'iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAYAAAC8/X7cAAAARUlEQVR4nO3PsQ0AIAwDQW9AxQDZf0jTAxGVG/SWvvfJKieTZzgAAAAAAAAAwKVtALqG7ETnFAnAG5I5DgAAAAAAAHwBWKJIfucQ1079AAAAAElFTkSuQmCC'
$fixture = Join-Path ([IO.Path]::GetTempPath()) 'lyrel-iterm2-276.png'
$png = [Convert]::FromBase64String($pngBase64)
$sha = [Security.Cryptography.SHA256]::Create()
if ([Convert]::ToHexString($sha.ComputeHash($png)) -ne $fixtureSha) {
    throw 'iTerm2 PNG fixture SHA-256 mismatch'
}
[IO.File]::WriteAllBytes($fixture, $png)
$windowsPath = $fixture -replace '\\', '/'
if ($windowsPath -notmatch '^([A-Za-z]):/(.*)$') {
    throw "expected an absolute drive path: $fixture"
}
$panePath = "/mnt/$($matches[1].ToLowerInvariant())/$($matches[2])"

$imgcatSha = (& wsl.exe -d Ubuntu-26.04 -- sha256sum $Imgcat) -split '\s+' | Select-Object -First 1
if ($imgcatSha.ToUpperInvariant() -ne '56F39F8C730EB7A64FED2BDB9E2C0BD2448F72E822CAF5B4B17C68A76DA52AF8') {
    throw "official imgcat SHA-256 mismatch: $imgcatSha"
}

function Invoke-Imgcat([string[]]$Options) {
    # Legacy `-l` is the single bounded `File=` form #276 admits; the script's
    # default `MultipartFile` form is the Refusal scene's unsupported input.
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    foreach ($argument in @('-d', 'Ubuntu-26.04', '--', 'bash', $Imgcat) + $Options + @($panePath)) {
        $start.ArgumentList.Add($argument)
    }
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $producer = [Diagnostics.Process]::Start($start)
    $producer.StandardOutput.BaseStream.CopyTo($out)
    $out.Flush()
    $producer.WaitForExit()
    if ($producer.ExitCode -ne 0) {
        throw "imgcat exited $($producer.ExitCode)"
    }
    $producer.Dispose()
}

$esc = [char]27
Write-Vt "${esc}[2J${esc}[HITERM2_276_$Scene`r`n"
Write-Vt "Official imgcat legacy File= producer; static PNG, then AFTER_ITERM2_276`r`n"
Write-Vt "${esc}[6;5H"
switch ($Scene) {
    'Cells' { Invoke-Imgcat @('-l', '-W', '4', '-H', '2') }
    'Pixels' { Invoke-Imgcat @('-l', '-W', '100px', '-H', '24px') }
    'Percent' { Invoke-Imgcat @('-l', '-W', '50%') }
    'Stretch' { Invoke-Imgcat @('-l', '-s', '-W', '8', '-H', '3') }
    'Scroll' {
        Write-Vt "${esc}[999;5H"
        Invoke-Imgcat @('-l')
    }
    'Refusal' {
        # The default producer form is MultipartFile/FilePart/FileEnd and must
        # be dropped; `inline=0` is a download request, and the last payload is
        # a PDF rather than a PNG. None may produce pixels or a cursor move.
        Invoke-Imgcat @()
        Write-Vt "${esc}]1337;File=inline=0;name=Zm9v:$pngBase64$([char]7)"
        Write-Vt "${esc}]1337;File=inline=1;name=Zm9v:JVBERi0xLjcK$([char]7)"
    }
    default { Invoke-Imgcat @('-l') }
}
Write-Vt 'AFTER_ITERM2_276'
if ($Scene -eq 'Clear') {
    Write-Vt "${esc}[2J${esc}[HCLEARED_ITERM2_276"
}
if ($KeepVisible) {
    [void](Read-Host 'Capture the owned candidate pane; press Enter to finish')
}
Write-Vt "`r`nITERM2_276_SCENE_DONE`r`n"

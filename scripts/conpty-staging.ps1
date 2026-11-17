# Shared by the Cargo wrapper and its native test runner. Integrity is always
# checked against the repository pin; file size and timestamps are not trust.
function Get-PinnedConptyFiles {
    $Root = Split-Path $PSScriptRoot -Parent
    $Manifest = Get-Content -LiteralPath (Join-Path $Root 'assets/windows/conpty/manifest.json') -Raw | ConvertFrom-Json
    $Prepared = if ($env:LYREL_CONPTY_DIR) { $env:LYREL_CONPTY_DIR } else { '.conpty' }
    if (-not [IO.Path]::IsPathRooted($Prepared)) { $Prepared = Join-Path $Root $Prepared }
    $Files = @(
        @{ Name = 'conpty.dll'; Source = Join-Path $Prepared 'x64/conpty.dll'; Hash = $Manifest.files.x64.'conpty.dll' },
        @{ Name = 'OpenConsole.exe'; Source = Join-Path $Prepared 'x64/OpenConsole.exe'; Hash = $Manifest.files.x64.'OpenConsole.exe' },
        @{ Name = 'LICENSE-ConPTY.txt'; Source = Join-Path $Prepared 'LICENSE-ConPTY.txt'; Hash = $Manifest.licenseSha256 }
    )
    foreach ($File in $Files) {
        if (-not (Test-Path -LiteralPath $File.Source -PathType Leaf) -or
            (Get-FileHash -LiteralPath $File.Source -Algorithm SHA256).Hash -ne $File.Hash) {
            throw "missing or mismatched ConPTY file: $($File.Source); run pwsh scripts/prepare-conpty.ps1"
        }
        $File.Length = (Get-Item -LiteralPath $File.Source).Length
    }
    return $Files
}

function Copy-PinnedConptyFiles {
    param([string]$Directory, [object[]]$Files)
    # Each caller deduplicates directories. Never replace a matching loaded DLL.
    foreach ($File in $Files) {
        $Destination = Join-Path $Directory $File.Name
        if ((Test-Path -LiteralPath $Destination -PathType Leaf) -and
            (Get-Item -LiteralPath $Destination).Length -eq $File.Length -and
            (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -eq $File.Hash) {
            continue
        }
        Copy-Item -LiteralPath $File.Source -Destination $Destination -Force
        if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -ne $File.Hash) {
            throw "staged ConPTY SHA-256 mismatch: $Destination"
        }
    }
}

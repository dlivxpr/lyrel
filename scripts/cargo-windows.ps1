# Runs Cargo and stages the pinned ConPTY set beside its reported executables.
# No assumptions about Cargo's internal build/deps directory layout.
# Usage: pwsh scripts/cargo-windows.ps1 build|test [Cargo arguments]
$ErrorActionPreference = 'Stop'
$Command = $args[0]
if ($Command -notin @('build', 'test')) { throw 'expected build or test' }
$CargoArgs = @($args | Select-Object -Skip 1)
$Separator = [Array]::IndexOf($CargoArgs, '--')
[string[]]$CompileArgs = if ($Separator -ge 0) { @($CargoArgs | Select-Object -First $Separator) } else { $CargoArgs }
$NoRun = $CompileArgs -contains '--no-run'
if ($Command -eq 'test' -and -not $NoRun) { $CompileArgs += '--no-run' }
$CompileArgs += '--message-format=json-render-diagnostics'

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
}

$Messages = [IO.Path]::GetTempFileName()
try {
    & cargo $Command @CompileArgs > $Messages
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $Directories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($Line in [IO.File]::ReadLines($Messages)) {
        # Cargo cannot suppress arbitrary procedural-macro stdout.
        if (-not $Line.StartsWith('{')) { Write-Host $Line; continue }
        $Message = $Line | ConvertFrom-Json
        if ($Message.reason -eq 'compiler-artifact' -and $Message.executable) {
            [void]$Directories.Add((Split-Path $Message.executable -Parent))
        }
    }
    foreach ($Directory in $Directories) {
        foreach ($File in $Files) {
            $Destination = Join-Path $Directory $File.Name
            # Leave matching, potentially loaded DLLs untouched.
            if ((Test-Path -LiteralPath $Destination) -and
                (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -eq $File.Hash) { continue }
            Copy-Item -LiteralPath $File.Source -Destination $Destination -Force
            if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -ne $File.Hash) {
                throw "staged ConPTY SHA-256 mismatch: $Destination"
            }
        }
    }
} finally {
    Remove-Item -LiteralPath $Messages -Force
}
if ($Command -eq 'test' -and -not $NoRun) {
    & cargo test @CargoArgs
    exit $LASTEXITCODE
}

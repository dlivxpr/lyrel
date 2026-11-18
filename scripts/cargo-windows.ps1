# Streams Cargo diagnostics and stages the pinned ConPTY beside reported binaries.
# Usage: pwsh scripts/cargo-windows.ps1 build|test|t-lib|t-core|... [Cargo arguments]
#        pwsh scripts/cargo-windows.ps1 nextest run [nextest arguments]
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'conpty-staging.ps1')
$Command = $args[0]
$TestCommands = @('test', 't-protocol', 't-core', 't-lib', 't-editor', 't-cli', 't-native-md', 't-native-term')
if ($Command -notin ($TestCommands + @('build', 't-check', 'nextest'))) {
    throw 'expected build, test, a standard t-* alias, or nextest run'
}
$CargoArgs = @($args | Select-Object -Skip 1)
$Files = @(Get-PinnedConptyFiles)
$Directories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)

if ($Command -eq 'nextest') {
    if ($CargoArgs[0] -ne 'run') { throw 'expected nextest run [arguments]' }
    $NextestArgs = @($CargoArgs | Select-Object -Skip 1)
    # Build only. Test discovery cannot run until the reported paths are staged.
    # Reuse nextest's documented binary metadata instead of compiling twice or
    # placing a PowerShell runner on every individual nextest test process.
    $ValueOptions = @('-p', '--package', '--exclude', '--test', '--bin', '--example', '--bench', '--features', '-F', '--target', '--target-dir', '--manifest-path', '--config', '-j', '--jobs', '--build-jobs', '--cargo-profile')
    $FlagOptions = @('--workspace', '--all', '--lib', '--bins', '--tests', '--examples', '--benches', '--all-targets', '--all-features', '--no-default-features', '--release', '--locked', '--frozen', '--offline', '--ignore-rust-version', '--future-incompat-report')
    $BuildArgs = @()
    $RunArgs = @()
    for ($Index = 0; $Index -lt $NextestArgs.Count; $Index++) {
        $Argument = $NextestArgs[$Index]
        if ($Argument -eq '--') {
            $RunArgs += $NextestArgs[$Index..($NextestArgs.Count - 1)]
            break
        }
        $Option = ($Argument -split '=', 2)[0]
        if ($Option -cin $ValueOptions) {
            $BuildArgs += $Argument
            if ($Option -in @('--manifest-path', '--config')) { $RunArgs += $Argument }
            if ($Argument -notmatch '=') {
                if (++$Index -ge $NextestArgs.Count) { throw "missing value for $Option" }
                $BuildArgs += $NextestArgs[$Index]
                if ($Option -in @('--manifest-path', '--config')) { $RunArgs += $NextestArgs[$Index] }
            }
        } elseif ($Option -cin $FlagOptions -or $Argument -cmatch '^-[pFj].+') {
            $BuildArgs += $Argument
        } else {
            $RunArgs += $Argument
        }
    }
    $Json = & cargo nextest list @BuildArgs --list-type binaries-only --message-format json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $Metadata = ($Json -join "`n") | ConvertFrom-Json
    foreach ($Binary in $Metadata.'rust-binaries'.PSObject.Properties.Value) {
        [void]$Directories.Add((Split-Path $Binary.'binary-path' -Parent))
    }
    foreach ($Package in $Metadata.'rust-build-meta'.'non-test-binaries'.PSObject.Properties.Value) {
        foreach ($Binary in $Package) {
            $BinaryPath = Join-Path $Metadata.'rust-build-meta'.'target-directory' $Binary.path
            [void]$Directories.Add((Split-Path $BinaryPath -Parent))
        }
    }
    foreach ($Directory in $Directories) { Copy-PinnedConptyFiles -Directory $Directory -Files $Files }
    if ($NextestArgs -contains '--no-run') { exit 0 }
    $MetadataFile = [IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllText($MetadataFile, ($Json -join "`n"), [Text.UTF8Encoding]::new($false))
        & cargo nextest run @RunArgs --binaries-metadata $MetadataFile
        exit $LASTEXITCODE
    } finally {
        Remove-Item -LiteralPath $MetadataFile -Force
    }
}

$Separator = [Array]::IndexOf($CargoArgs, '--')
[string[]]$CompileArgs = if ($Separator -ge 0) { @($CargoArgs | Select-Object -First $Separator) } else { $CargoArgs }
$TestArgs = if ($Separator -ge 0) { @($CargoArgs | Select-Object -Skip ($Separator + 1)) } else { @() }
if ($CompileArgs | Where-Object { $_ -match '^--message-format($|=)' }) {
    throw 'the Windows wrapper owns --message-format for executable discovery'
}
if ($Command -in $TestCommands -and $CompileArgs -notcontains '--no-run') {
    # Cargo retains its own cwd, filtering, doctest and failure semantics. An
    # argument-array config preserves spaces in the runner path on Windows.
    $Runner = @('pwsh', '-NoProfile', '-File', (Join-Path $PSScriptRoot 'cargo-windows-runner.ps1')) | ConvertTo-Json -Compress
    $CompileArgs += @('--config', "target.x86_64-pc-windows-msvc.runner=$Runner")
}
$CompileArgs += '--message-format=json'
if ($Separator -ge 0) { $CompileArgs += @('--') + $TestArgs }
& cargo $Command @CompileArgs | ForEach-Object {
    $Line = $_
    if ($Line.StartsWith('{')) {
        try { $Message = $Line | ConvertFrom-Json } catch { $Message = $null }
        if ($Message.reason -eq 'compiler-artifact') {
            if ($Message.executable) { [void]$Directories.Add((Split-Path $Message.executable -Parent)) }
        } elseif ($Message.reason -eq 'compiler-message') {
            if ($Message.message.rendered) { [Console]::Error.Write($Message.message.rendered) }
        } elseif ($Message.reason -notin @('build-script-executed', 'build-finished')) {
            Write-Host $Line
        }
    } else {
        Write-Host $Line
    }
}
$CargoExit = $LASTEXITCODE
if ($CargoExit -ne 0) { exit $CargoExit }
foreach ($Directory in $Directories) { Copy-PinnedConptyFiles -Directory $Directory -Files $Files }
exit 0

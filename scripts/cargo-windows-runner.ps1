# Cargo's native target runner: stage before execution, not asynchronously when
# compiler-artifact stdout happens to be consumed by the parent PowerShell pipe.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'conpty-staging.ps1')
if ($args.Count -eq 0) { throw 'expected a test executable' }
$Executable = [IO.Path]::GetFullPath($args[0])
$TestArgs = @($args | Select-Object -Skip 1)
Copy-PinnedConptyFiles -Directory (Split-Path $Executable -Parent) -Files @(Get-PinnedConptyFiles)
& $Executable @TestArgs
exit $LASTEXITCODE

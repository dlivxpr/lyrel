# Prepares the pinned Microsoft ConPTY pair for development and packaging.
#
#   pwsh scripts/prepare-conpty.ps1                 # download (once), verify, stage into .conpty/
#   pwsh scripts/prepare-conpty.ps1 -Offline        # verify + stage from the cached nupkg, or
#                                                   # verify an already-prepared tree as-is
#
# Source: the official `Microsoft.Windows.Console.ConPTY` nupkg on nuget.org,
# pinned by assets/windows/conpty/manifest.json (package version, nupkg
# SHA-512 taken from the NuGet catalog's packageHash, per-file SHA-256, PE
# file version). See assets/windows/conpty/README.md for why the pair is
# sideloaded at all.
#
# The prepared directory layout mirrors the release payload:
#   <dir>/x64/{conpty.dll, OpenConsole.exe}
#   <dir>/LICENSE-ConPTY.txt   (the MIT notice, staged under its release name)
#   <dir>/manifest.json        (copy of the manifest this tree was checked against)
#
# Consumers resolve the directory in this order:
#   1. -PreparedDir parameter         (this script only)
#   2. LYREL_CONPTY_DIR env var       (this script, build.rs, bundle-windows.ps1)
#   3. <repo>/.conpty                 (per-worktree default; gitignored)
# The nupkg cache works the same way through LYREL_CONPTY_NUPKG /
# <repo>/.conpty-cache/<package>.nupkg, so several worktrees can share one
# prepared tree and one download.
#
# Everything this script stages is verified before it is moved into place;
# a file that fails verification never reaches the prepared directory.
[CmdletBinding()]
param(
    [string]$PreparedDir = $env:LYREL_CONPTY_DIR,
    [string]$NupkgCache = $env:LYREL_CONPTY_NUPKG,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path $PSScriptRoot -Parent
$ManifestPath = Join-Path $Root 'assets/windows/conpty/manifest.json'
$Manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
$Version = $Manifest.packageVersion

if (-not $PreparedDir) { $PreparedDir = Join-Path $Root '.conpty' }
$PreparedDir = [System.IO.Path]::GetFullPath($PreparedDir)
if (-not $NupkgCache) { $NupkgCache = Join-Path $Root '.conpty-cache' }
if ((Test-Path -LiteralPath $NupkgCache -PathType Container) -or ($NupkgCache -notmatch '\.nupkg$')) {
    $NupkgCache = Join-Path $NupkgCache "microsoft.windows.console.conpty.$Version.nupkg"
}

function Assert-True([bool]$ok, [string]$message) {
    if (-not $ok) { throw "prepare-conpty: $message" }
}

function Test-StagedFile([string]$Path, [string]$ExpectedSha256, [string]$Label) {
    Assert-True (Test-Path -LiteralPath $Path -PathType Leaf) `
        "$Label is missing: $Path — re-run: pwsh scripts/prepare-conpty.ps1"
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
    Assert-True ($actual -eq $ExpectedSha256) `
        "$Label SHA-256 mismatch: manifest expects $ExpectedSha256, file is $actual ($Path) — re-run: pwsh scripts/prepare-conpty.ps1"
}

function Test-StagedVersion([string]$Path, [string]$Label) {
    $ver = (Get-Item -LiteralPath $Path).VersionInfo.FileVersion
    Assert-True ($ver -eq $Manifest.fileVersion) `
        "$Label reports FileVersion '$ver', manifest requires '$($Manifest.fileVersion)' ($Path) — re-run: pwsh scripts/prepare-conpty.ps1"
    $sig = Get-AuthenticodeSignature -LiteralPath $Path
    Assert-True ($sig.Status -eq 'Valid') `
        "$Label has no valid Authenticode signature (status $($sig.Status)): $Path — re-run: pwsh scripts/prepare-conpty.ps1"
    return $ver
}

# ---- 1. Get the nupkg: cache, download, or prepared-tree-only reuse ---------
$havePkg = Test-Path -LiteralPath $NupkgCache -PathType Leaf
if (-not $havePkg -and -not $Offline) {
    New-Item -ItemType Directory -Force -Path (Split-Path $NupkgCache -Parent) | Out-Null
    $tmp = "$NupkgCache.download"
    Write-Host "downloading $($Manifest.nupkgUrl)"
    Invoke-WebRequest -Uri $Manifest.nupkgUrl -OutFile $tmp
    Move-Item -Force $tmp $NupkgCache
    $havePkg = $true
}

if ($havePkg) {
    # Independent-source check: the nupkg must hash to the SHA-512 the NuGet
    # catalog published as this version's packageHash (pinned in the manifest).
    $pkgHash = [Convert]::ToBase64String(
        [System.Security.Cryptography.SHA512]::Create().ComputeHash(
            [System.IO.File]::ReadAllBytes($NupkgCache)))
    Assert-True ($pkgHash -eq $Manifest.nupkgSha512) `
        "nupkg SHA-512 mismatch: manifest pins $($Manifest.nupkgSha512), file is $pkgHash ($NupkgCache) — delete the cache and re-run: pwsh scripts/prepare-conpty.ps1"

    # ---- 2. Extract the per-arch pair to a scratch dir ----------------------
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Scratch = Join-Path ([System.IO.Path]::GetTempPath()) "lyrel-conpty-$([guid]::NewGuid())"
    New-Item -ItemType Directory -Path $Scratch | Out-Null
    try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($NupkgCache)
        try {
            foreach ($arch in @('x64')) {
                foreach ($file in @('conpty.dll', 'OpenConsole.exe')) {
                    $entryName = if ($file -eq 'conpty.dll') {
                        "runtimes/win-$arch/native/conpty.dll"
                    } else {
                        "build/native/runtimes/$arch/OpenConsole.exe"
                    }
                    $entry = $zip.Entries | Where-Object { $_.FullName -eq $entryName }
                    Assert-True ($null -ne $entry) "nupkg $NupkgCache does not contain $entryName"
                    New-Item -ItemType Directory -Force -Path (Join-Path $Scratch $arch) | Out-Null
                    $out = Join-Path $Scratch "$arch/$file"
                    $src = $entry.Open(); $dst = [System.IO.File]::Create($out)
                    try { $src.CopyTo($dst) } finally { $dst.Dispose(); $src.Dispose() }
                }
            }
        } finally { $zip.Dispose() }
        # Verify the prepared architecture before changing the prepared tree.
        foreach ($arch in @('x64')) {
            $expected = $Manifest.files.$arch
            Test-StagedFile (Join-Path $Scratch "$arch/conpty.dll") $expected.'conpty.dll' "conpty.dll ($arch)"
            Test-StagedFile (Join-Path $Scratch "$arch/OpenConsole.exe") $expected.'OpenConsole.exe' "OpenConsole.exe ($arch)"
            $dllVer = Test-StagedVersion (Join-Path $Scratch "$arch/conpty.dll") "conpty.dll ($arch)"
            $exeVer = Test-StagedVersion (Join-Path $Scratch "$arch/OpenConsole.exe") "OpenConsole.exe ($arch)"
            Assert-True ($dllVer -eq $exeVer) `
                "ConPTY pair mismatch ($arch): conpty.dll is $dllVer, OpenConsole.exe is $exeVer — the package is inconsistent, do not ship it"
        }
        foreach ($arch in @('x64')) {
            New-Item -ItemType Directory -Force -Path (Join-Path $PreparedDir $arch) | Out-Null
            foreach ($file in @('conpty.dll', 'OpenConsole.exe')) {
                Move-Item -Force (Join-Path $Scratch "$arch/$file") (Join-Path $PreparedDir "$arch/$file")
            }
        }
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $Scratch
    }
} else {
    # Offline reuse: the prepared tree itself is the input. It still gets the
    # full verification below — absence of the nupkg is not a skip.
    Write-Host "no nupkg at $NupkgCache — verifying the already-prepared tree at $PreparedDir"
}

# ---- 4. The MIT notice ships under its release name --------------------------
$LicenseSrc = Join-Path $Root 'assets/windows/conpty/LICENSE.txt'
Test-StagedFile $LicenseSrc $Manifest.licenseSha256 'assets/windows/conpty/LICENSE.txt'
New-Item -ItemType Directory -Force -Path $PreparedDir | Out-Null
Copy-Item -Force $LicenseSrc (Join-Path $PreparedDir 'LICENSE-ConPTY.txt')

# ---- 5. Verify the final prepared tree end to end ---------------------------
# A tree that only ever passed step 3 still gets re-checked here, so an
# offline reuse run is a full verification, not a presence check.
foreach ($arch in @('x64')) {
    $expected = $Manifest.files.$arch
    Test-StagedFile (Join-Path $PreparedDir "$arch/conpty.dll") $expected.'conpty.dll' "prepared conpty.dll ($arch)"
    Test-StagedFile (Join-Path $PreparedDir "$arch/OpenConsole.exe") $expected.'OpenConsole.exe' "prepared OpenConsole.exe ($arch)"
    $dllVer = Test-StagedVersion (Join-Path $PreparedDir "$arch/conpty.dll") "prepared conpty.dll ($arch)"
    $exeVer = Test-StagedVersion (Join-Path $PreparedDir "$arch/OpenConsole.exe") "prepared OpenConsole.exe ($arch)"
    Assert-True ($dllVer -eq $exeVer) `
        "prepared ConPTY pair mismatch ($arch): conpty.dll is $dllVer, OpenConsole.exe is $exeVer — re-run: pwsh scripts/prepare-conpty.ps1"
}
Test-StagedFile (Join-Path $PreparedDir 'LICENSE-ConPTY.txt') $Manifest.licenseSha256 'prepared LICENSE-ConPTY.txt'
Copy-Item -Force $ManifestPath (Join-Path $PreparedDir 'manifest.json')

Write-Host "prepared Microsoft.Windows.Console.ConPTY $Version (file version $($Manifest.fileVersion)) at $PreparedDir"
foreach ($arch in @('x64')) {
    foreach ($file in @('conpty.dll', 'OpenConsole.exe')) {
        $p = Join-Path $PreparedDir "$arch/$file"
        "{0}  {1}  {2}" -f $arch, $file, (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash
    }
}

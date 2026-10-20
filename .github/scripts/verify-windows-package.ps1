# Verifies that the Windows release artifacts carry everything the in-app
# updater requires, immediately after bundle-windows.ps1 produces them.
#
# The updater refuses to install a package it cannot recognise, and it does so
# on the user's machine, after the download, after the GUI has exited. Every
# fact it checks there is checked here instead, so a packaging mistake fails
# the release build rather than every user's next update.
#
# Mirrors, in order:
#   core::update::windows_update_layout_for      — the install marker
#   core::update::package_for_current_install    — lyrel-updater.exe beside the app
#   lyrel-updater `windows::verify_portable_payload`  — portable layout + versions
#   lyrel-updater `windows::extract_portable_archive` — ZIP entry rules
#   lyrel-updater `windows::verify_file_version`      — setup.exe PE version
#
# 在仓库根目录运行：& ./.github/scripts/verify-windows-package.ps1 <arch> [version]
# version 默认使用根 Cargo.toml，与打包脚本相同。
$ErrorActionPreference = 'Stop'

$Arch = $args[0]
if (-not $Arch) { throw "usage: verify-windows-package.ps1 <arch> [version]" }
$Version = $args[1]
if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
}
if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "verify-windows-package: expected a stable version (got '$Version')"
}
$ServerAsset = @{
    'x86_64' = 'lyrel-server-linux-x86_64-musl'
}[$Arch]
if (-not $ServerAsset) { throw "unsupported Windows package architecture: $Arch" }
$ExpectedMachine = @{ 'x86_64' = 0x8664 }[$Arch]
# The PE fixed-version resource carries only numeric components, so the updater
# compares the release version's numeric core against it. Keep the same split.
$VersionCore = ($Version -split '[-+]', 2)[0]

$Name  = "lyrel-$Version-windows-$Arch"
$Zip   = "dist/$Name.zip"
$Setup = "dist/$Name-setup.exe"
$Stage = "dist/$Name"

$failures = New-Object System.Collections.Generic.List[string]
function Fail([string]$message) { $failures.Add($message) }

function Get-ProductVersion([string]$path) {
    # The same string the updater reads back with VerQueryValueW
    # (\StringFileInfo\<lang><cp>\ProductVersion).
    (Get-Item -LiteralPath $path).VersionInfo.ProductVersion
}

function Assert-BinaryVersion([string]$path, [string]$label) {
    if (-not (Test-Path -LiteralPath $path)) { Fail "$label is missing: $path"; return }
    $actual = Get-ProductVersion $path
    if ($actual -ne $Version) {
        Fail "$label reports ProductVersion '$actual', expected '$Version'"
    }
}

# The CLI has no Windows version-resource contract. Check all three payload
# executables by their PE Machine field, but keep version checks to GUI/updater.
# Do not apply this to setup.exe: Inno's bootstrapper can legitimately be x86.
function Assert-BinaryArchitecture([string]$path, [string]$label) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { Fail "$label is missing: $path"; return }
    $stream = [System.IO.File]::OpenRead((Get-Item -LiteralPath $path).FullName)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 0x40 -or $reader.ReadUInt16() -ne 0x5A4D) {
            Fail "$label is not a PE file (no MZ header)"
            return
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 24)) {
            Fail "$label has an invalid PE header offset"
            return
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            Fail "$label is not a PE file (no PE signature)"
            return
        }
        $machine = $reader.ReadUInt16()
        if ($machine -ne $ExpectedMachine) {
            Fail ("$label has PE Machine 0x{0:X4}, expected 0x{1:X4} for {2}" -f $machine, $ExpectedMachine, $Arch)
        }
    } finally {
        $reader.Dispose()
    }
}

# Microsoft ships conpty.dll and OpenConsole.exe as one supported unit. Check
# the pinned architecture, version and bytes in the actual package, including
# its MIT notice; a same-version damaged pair is not a valid payload.
$ConptyManifest = Get-Content -LiteralPath 'assets/windows/conpty/manifest.json' -Raw | ConvertFrom-Json
$ConptyArch = @{ 'x86_64' = 'x64' }[$Arch]
function Assert-ConptyPair([string]$directory, [string]$label) {
    $manifestPath = Join-Path $directory 'ConPTY-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        Fail "$label is missing the target ConPTY manifest"
    } elseif ((Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash -ne
              (Get-FileHash -LiteralPath 'assets/windows/conpty/manifest.json' -Algorithm SHA256).Hash) {
        Fail "$label carries a target ConPTY manifest that differs from the release pin"
    }
    $versions = @{}
    foreach ($file in 'conpty.dll', 'OpenConsole.exe') {
        $path = Join-Path $directory $file
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Fail "$label is missing the bundled $file"
            return
        }
        $versions[$file] = (Get-Item -LiteralPath $path).VersionInfo.FileVersion
        if ($versions[$file] -ne $ConptyManifest.fileVersion) {
            Fail "$label carries $file version '$($versions[$file])', expected '$($ConptyManifest.fileVersion)'"
        }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $ConptyManifest.files.$ConptyArch.$file) {
            Fail "$label carries $file with a SHA-256 mismatch"
        }
    }
    if ($versions['conpty.dll'] -ne $versions['OpenConsole.exe']) {
        Fail ("$label carries a mismatched ConPTY pair: conpty.dll is " +
              "$($versions['conpty.dll']), OpenConsole.exe is $($versions['OpenConsole.exe'])")
    }
    if (-not (Test-Path -LiteralPath (Join-Path $directory 'LICENSE-ConPTY.txt') -PathType Leaf)) {
        Fail "$label ships the bundled ConPTY without its MIT notice"
    } elseif ((Get-FileHash -LiteralPath (Join-Path $directory 'LICENSE-ConPTY.txt') -Algorithm SHA256).Hash -ne $ConptyManifest.licenseSha256) {
        Fail "$label carries a ConPTY MIT notice with a SHA-256 mismatch"
    }
}

# A shipped binary that imports VCRUNTIME140.dll does not start on a Windows
# machine that has never installed the Visual C++ Redistributable: the loader
# fails before `main`, with no log and no window. That is #902, and it is how
# 26.8.2 failed winget's install validation
# (microsoft/winget-pkgs#415841) while running perfectly on every machine that
# had ever seen Visual Studio. `.cargo/config.toml` links the CRT statically;
# checked here, on the payload that actually ships, because neither the build
# nor the tests can observe the difference.
function Assert-NoVcRuntime([string]$directory, [string]$label) {
    try {
        & (Join-Path $PSScriptRoot 'assert-no-vcruntime.ps1') $directory
    } catch {
        Fail "$label would not start without the VC++ Redistributable: $($_.Exception.Message)"
    }
}

# ---- Portable ZIP --------------------------------------------------------
# Update rules live in the updater's extractor; the ones that can be broken by
# packaging alone are re-stated here.
if (-not (Test-Path -LiteralPath $Zip)) {
    Fail "the portable archive is missing: $Zip"
} else {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $Zip).Path)
    try {
        $entries = @($archive.Entries | ForEach-Object { $_.FullName })
    } finally {
        $archive.Dispose()
    }

    # `extract_portable_archive` rejects a backslash outright: the ZIP spec
    # names '/' as the separator, and a mixed archive is one the updater will
    # not unpack. PowerShell's archive writer has emitted both over the years.
    $backslashed = @($entries | Where-Object { $_.Contains('\') })
    if ($backslashed.Count -gt 0) {
        Fail ("the portable archive uses backslash separators the updater rejects: " +
              ($backslashed -join ', '))
    }

    # `validate_portable_relative_path` allows only these top-level names.
    $managed = @(
        'lyrel-app.exe', 'lyrel.exe', 'lyrel-updater.exe', '.lyrel-portable',
        'completions', 'server', 'LICENSE.txt', 'README.md',
        'conpty.dll', 'OpenConsole.exe', 'LICENSE-ConPTY.txt', 'ConPTY-manifest.json'
    )
    $roots = @($entries |
        ForEach-Object { ($_ -split '[\\/]', 2)[0] } |
        Sort-Object -Unique)
    foreach ($root in $roots) {
        if ($managed -notcontains $root) {
            Fail "the portable archive has a top-level entry the updater rejects: $root"
        }
    }

    # The Inno marker and the portable marker are mutually exclusive: whichever
    # one is present decides how the updater replaces this installation.
    if ($entries -contains '.lyrel-inno-install') {
        Fail "the portable archive carries the Inno install marker"
    }

    $unzipped = Join-Path ([System.IO.Path]::GetTempPath()) "lyrel-verify-portable-$([guid]::NewGuid())"
    New-Item -ItemType Directory -Force -Path $unzipped | Out-Null
    try {
        [System.IO.Compression.ZipFile]::ExtractToDirectory(
            (Resolve-Path $Zip).Path, $unzipped)

        # `verify_portable_payload`: every required member, then the marker
        # content, then the complete version of both executables.
        foreach ($required in @('lyrel-app.exe', 'lyrel.exe', 'lyrel-updater.exe',
                                '.lyrel-portable', 'LICENSE.txt', 'README.md', "server/$ServerAsset")) {
            if (-not (Test-Path -LiteralPath (Join-Path $unzipped $required) -PathType Leaf)) {
                Fail "the portable archive is missing the required file $required"
            }
        }
        if (-not (Test-Path -LiteralPath (Join-Path $unzipped 'completions') -PathType Container)) {
            Fail "the portable archive is missing the required directory completions"
        }

        $markerPath = Join-Path $unzipped '.lyrel-portable'
        if (Test-Path -LiteralPath $markerPath) {
            $marker = [System.IO.File]::ReadAllBytes($markerPath)
            $expected = [System.Text.Encoding]::ASCII.GetBytes('portable-v1')
            if (@(Compare-Object $marker $expected -SyncWindow 0).Count -ne 0) {
                Fail "the portable marker does not contain exactly 'portable-v1'"
            }
        }

        Assert-BinaryVersion (Join-Path $unzipped 'lyrel-app.exe') 'the portable lyrel-app.exe'
        Assert-BinaryVersion (Join-Path $unzipped 'lyrel-updater.exe') 'the portable lyrel-updater.exe'
        foreach ($binary in 'lyrel-app.exe', 'lyrel.exe', 'lyrel-updater.exe') {
            Assert-BinaryArchitecture (Join-Path $unzipped $binary) "the portable $binary"
        }
        Assert-ConptyPair $unzipped 'the portable archive'
        Assert-NoVcRuntime $unzipped 'the portable archive'
    } finally {
        Remove-Item -Recurse -Force $unzipped -ErrorAction SilentlyContinue
    }
}

# ---- Inno payload --------------------------------------------------------
# ISCC compiled the installer from this staging directory, so what it holds is
# what lands in {app}. Reading the compiled setup.exe back would need
# innoextract, which the runners do not carry.
if (-not (Test-Path -LiteralPath $Stage -PathType Container)) {
    Fail "the Inno staging directory is missing: $Stage"
} else {
    $markerPath = Join-Path $Stage '.lyrel-inno-install'
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        Fail "the Inno payload is missing the .lyrel-inno-install marker; installed copies would never be offered an in-app update"
    } else {
        $marker = [System.IO.File]::ReadAllBytes((Get-Item -LiteralPath $markerPath).FullName)
        $expected = [System.Text.Encoding]::ASCII.GetBytes('inno-v1')
        if ($marker.Length -ne $expected.Length -or
            @(Compare-Object $marker $expected -SyncWindow 0).Count -ne 0) {
            Fail "the Inno marker does not contain exactly 'inno-v1'"
        }
    }
    if (Test-Path -LiteralPath (Join-Path $Stage '.lyrel-portable')) {
        Fail "the Inno payload carries the portable marker, which would misroute the updater"
    }
    foreach ($required in @('lyrel-app.exe', 'lyrel.exe', 'lyrel-updater.exe',
                            'LICENSE.txt', 'README.md', "server/$ServerAsset")) {
        if (-not (Test-Path -LiteralPath (Join-Path $Stage $required) -PathType Leaf)) {
            Fail "the Inno payload is missing the required file $required"
        }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Stage 'completions') -PathType Container)) {
        Fail "the Inno payload is missing the required directory completions"
    }
    Assert-BinaryVersion (Join-Path $Stage 'lyrel-app.exe') 'the installed lyrel-app.exe'
    Assert-BinaryVersion (Join-Path $Stage 'lyrel-updater.exe') 'the installed lyrel-updater.exe'
    foreach ($binary in 'lyrel-app.exe', 'lyrel.exe', 'lyrel-updater.exe') {
        Assert-BinaryArchitecture (Join-Path $Stage $binary) "the installed $binary"
    }
    Assert-ConptyPair $Stage 'the Inno payload'
    Assert-NoVcRuntime $Stage 'the Inno payload'
}

# ---- Setup executable ----------------------------------------------------
# `verify_update` re-reads this numeric version after the GUI exits and before
# it runs the installer, so a mis-stamped VersionInfoVersion is an update that
# aborts on the user's machine.
if (-not (Test-Path -LiteralPath $Setup -PathType Leaf)) {
    Fail "the Windows installer is missing: $Setup"
} else {
    $info = (Get-Item -LiteralPath $Setup).VersionInfo
    $actual = "$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart)"
    if ($actual -ne $VersionCore) {
        Fail "$Setup reports file version '$actual', expected '$VersionCore'"
    }
}

if ($failures.Count -gt 0) {
    foreach ($failure in $failures) { Write-Output "::error::$failure" }
    throw "the Windows release package would not be updatable in place ($($failures.Count) problem(s))"
}

Write-Output "Windows package verified: payload layout, markers, architectures and versions match $Version ($Arch)"

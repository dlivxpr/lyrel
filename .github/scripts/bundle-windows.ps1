# 在仓库根目录运行：& ./.github/scripts/bundle-windows.ps1 <target-triple> <arch-label>
# Package the release binary twice from one staged payload:
#   dist/lyrel-<version>-windows-<arch>.zip        portable (unzip anywhere)
#   dist/lyrel-<version>-windows-<arch>-setup.exe  Inno Setup installer
#     (Program Files or per-user, Start Menu shortcut, "Apps" uninstall entry)
#
# Fonts are embedded via include_bytes! and the app icon is compiled into the
# executable as a resource (see build.rs). So the payload is lyrel-app.exe plus a
# sibling completions\ dir (loaded at runtime — see terminal::signature), the
# bundled ConPTY pair (loaded by the DLL search path — see below) and the
# license/readme. Windows release artifacts are intentionally unsigned. The
# in-app updater verifies the published SHA-256 checksum and PE file version
# before and after waiting for the GUI to exit.
$ErrorActionPreference = 'Stop'

$Target = $args[0]
$Arch   = $args[1]
$Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "bundle-windows: expected a stable version in Cargo.toml (got '$Version')"
}
# stable 版本的数字核心同时用于 Inno AppVersion 和 PE 文件版本。
$VersionCore = ($Version -split '[-+]', 2)[0]
$VersionInfoVersion = "${VersionCore}.0"
$Name  = "lyrel-$Version-windows-$Arch"
$Stage = "dist/$Name"
$PackageUpdater = $env:LYREL_PACKAGE_UPDATE_HELPER -ne '0'

Remove-Item -Recurse -Force dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Copy-Item "target/$Target/release/lyrel-app.exe" "$Stage/lyrel-app.exe"
# The CLI, staged beside the GUI so both the zip and the installer carry it.
# `core::cli_install` resolves it relative to lyrel-app.exe and puts that
# directory on the user's PATH.
Copy-Item "target/$Target/release/lyrel.exe" "$Stage/lyrel.exe"
if ($PackageUpdater) {
    # The installed copy is never executed in place during an update. The GUI
    # first copies it to a private staging directory so Inno can replace every
    # installed executable without colliding with Windows image locks.
    Copy-Item "target/$Target/release/lyrel-updater.exe" "$Stage/lyrel-updater.exe"
}
New-Item -ItemType Directory -Force -Path "$Stage/completions" | Out-Null
Copy-Item "assets/completions/*.json" "$Stage/completions/"
Copy-Item "assets/completions/LICENSE-Fig.txt" "$Stage/completions/"
Copy-Item LICENSE "$Stage/LICENSE.txt"
Copy-Item README.md "$Stage/README.md"

# Microsoft's redistributable ConPTY, which `portable-pty` prefers over the
# in-box conhost when it sits beside the running executable — and the daemon is
# `lyrel-app.exe --daemon`, so beside it is exactly here. The in-box host
# swallows a pane's OSC 11 background query (#345); this pair forwards it. The
# pair is prepared out-of-band by scripts/prepare-conpty.ps1 (pinned nupkg,
# verified against assets/windows/conpty/manifest.json); packaging consumes
# that verified tree the same way the build does — via LYREL_CONPTY_DIR or the
# default .conpty/ — and never downloads anything itself. The two files are
# one unit, so a missing half or a pair that drifts from the pinned version is
# an error rather than a warning. See assets/windows/conpty/README.md.
$ConptyArch = @{ 'x86_64-pc-windows-msvc' = 'x64' }[$Target]
if (-not $ConptyArch) { throw "no prepared ConPTY for $Target - see assets/windows/conpty/README.md" }
$ConptyDir = if ($env:LYREL_CONPTY_DIR) { $env:LYREL_CONPTY_DIR } else { '.conpty' }
$ConptyManifest = Get-Content -LiteralPath 'assets/windows/conpty/manifest.json' -Raw | ConvertFrom-Json
foreach ($File in 'conpty.dll', 'OpenConsole.exe') {
    $Source = Join-Path $ConptyDir "$ConptyArch/$File"
    if (-not (Test-Path $Source)) { throw "missing prepared ConPTY file: $Source - run: pwsh scripts/prepare-conpty.ps1" }
    $SourceVersion = (Get-Item -LiteralPath $Source).VersionInfo.FileVersion
    if ($SourceVersion -ne $ConptyManifest.fileVersion) {
        throw "prepared $File reports FileVersion '$SourceVersion', manifest requires '$($ConptyManifest.fileVersion)' ($Source) - run: pwsh scripts/prepare-conpty.ps1"
    }
    $ExpectedHash = $ConptyManifest.files.$ConptyArch.$File
    if ((Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash -ne $ExpectedHash) {
        throw "prepared ConPTY SHA-256 mismatch: $Source - run: pwsh scripts/prepare-conpty.ps1"
    }
    Copy-Item $Source "$Stage/$File"
    if ((Get-FileHash -LiteralPath "$Stage/$File" -Algorithm SHA256).Hash -ne $ExpectedHash) {
        throw "staged ConPTY SHA-256 mismatch: $Stage/$File"
    }
}
$ConptyLicense = Join-Path $ConptyDir 'LICENSE-ConPTY.txt'
if (-not (Test-Path $ConptyLicense)) { throw "missing prepared ConPTY license: $ConptyLicense - run: pwsh scripts/prepare-conpty.ps1" }
if ((Get-FileHash -LiteralPath $ConptyLicense -Algorithm SHA256).Hash -ne $ConptyManifest.licenseSha256) {
    throw "prepared ConPTY MIT notice SHA-256 mismatch: $ConptyLicense - run: pwsh scripts/prepare-conpty.ps1"
}
Copy-Item $ConptyLicense "$Stage/LICENSE-ConPTY.txt"
# The archive checksum authenticates the target pin for older updater helpers.
Copy-Item 'assets/windows/conpty/manifest.json' "$Stage/ConPTY-manifest.json"

# WSL 只使用随包提供的同架构 Linux musl server，不回退下载。
# 包内路径必须是 server/<asset>，与 daemon::install::wsl 的发现契约一致。
$ServerAsset = @{
    'x86_64-pc-windows-msvc'  = 'lyrel-server-linux-x86_64-musl'
}[$Target]
if (-not $ServerAsset) { throw "no bundled WSL server asset known for $Target" }
$ServerSrc = "bundled-server/$ServerAsset"
if (-not (Test-Path -LiteralPath $ServerSrc -PathType Leaf)) {
    throw "missing bundled WSL server: $ServerSrc"
}
New-Item -ItemType Directory -Force -Path "$Stage/server" | Out-Null
Copy-Item $ServerSrc "$Stage/server/$ServerAsset"
Write-Host "OK bundled $ServerAsset"

# The marker tells the in-app updater which of the two Windows layouts it is
# running from, and therefore which release asset can replace it. It says
# nothing about where updates come from: that is always the latest stable
# release. The Inno payload gets the mutually exclusive marker below.
if ($PackageUpdater) {
    Set-Content -Path "$Stage/.lyrel-portable" -Value 'portable-v1' -NoNewline -Encoding ascii
}
Compress-Archive -Path "$Stage/*" -DestinationPath "dist/$Name.zip" -Force
if ($PackageUpdater) {
    # The Inno payload must never retain the mutually exclusive portable marker.
    Remove-Item -LiteralPath "$Stage/.lyrel-portable" -Force
    Set-Content -Path "$Stage/.lyrel-inno-install" -Value 'inno-v1' -NoNewline -Encoding ascii
}

# Inno's architecture identifier for this payload. `x64compatible` also admits
# ARM64 Windows, which runs the x64 build under emulation — the only Windows
# build the release publishes.
$InstallArch = @{
    'x86_64-pc-windows-msvc' = 'x64compatible'
}[$Target]
if (-not $InstallArch) { throw "no Inno architecture known for $Target" }

# Installer, built from the same staged payload. ISCC is on PATH on GitHub's
# windows-latest image; fall back to the default install location.
$Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $Iscc) { $Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
& $Iscc `
    "/DAppVersion=$Version" `
    "/DVersionInfoVersion=$VersionInfoVersion" `
    "/DStageDir=$((Resolve-Path $Stage).Path)" `
    "/DOutputDir=$((Resolve-Path dist).Path)" `
    "/DOutputName=$Name-setup" `
    "/DArchitectures=$InstallArch" `
    "$PSScriptRoot/windows-installer.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC exited with $LASTEXITCODE" }

# The staging directory stays. It is what ISCC compiled the installer from, and
# reading the compiled setup.exe back would need innoextract, which the runners
# do not carry — so `verify-windows-package.ps1` reads the payload here instead.
# It never reaches the release: both workflows upload named file globs
# (*.zip, *-setup.exe, …), and a directory matches none of them.
Write-Host "OK dist/$Name.zip"
Write-Host "OK dist/$Name-setup.exe"

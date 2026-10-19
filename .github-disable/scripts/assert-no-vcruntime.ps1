# Fail the build if a shipped Windows binary imports the Visual C++
# redistributable (#902).
#
# A default MSVC build links the CRT dynamically, which leaves VCRUNTIME140.dll
# (and, once any C++ is linked in, MSVCP140.dll) in the import table. Neither
# ships with Windows, so on a machine that has never installed the "Visual C++
# 2015-2022 Redistributable" the loader fails before `main` with
#
#     The code execution cannot proceed because VCRUNTIME140.dll was not found.
#
# That is what winget's install validation hit on 26.8.2
# (microsoft/winget-pkgs#415841). `.cargo/config.toml` fixes it by linking the
# CRT statically; this script is the guard that keeps it fixed, because the
# failure is invisible on any developer or CI machine — they all have the
# redistributable, installed by Visual Studio or by some other package.
#
# The `api-ms-win-crt-*` imports are a different thing and are fine: those are
# the Universal CRT, an in-box Windows component since Windows 10. They are
# only present at all when the CRT is linked dynamically, so a statically
# linked binary has none of these imports either.
#
# Usage: assert-no-vcruntime.ps1 <path> [<path> ...]
# Each path is a PE file, or a directory whose *.exe and *.dll are scanned.
# Microsoft's own redistributable ConPTY pair is skipped: it is prebuilt, we do
# not link it, and it is already statically linked against the CRT.
$ErrorActionPreference = 'Stop'

$Forbidden = @('vcruntime140', 'vcruntime140_1', 'msvcp140', 'msvcp140_1',
               'msvcp140_2', 'concrt140', 'vcamp140', 'vcomp140', 'msvcr120',
               'msvcr110', 'msvcr100')
# Not ours to link, and already CRT-static (verified with dumpbin /dependents).
$Skip = @('conpty.dll', 'OpenConsole.exe')

# Minimal PE import-table reader. Deliberately not `dumpbin`: that needs a
# Visual Studio installation on PATH, which is exactly the assumption this
# check exists to stop us making.
function Get-PEImportedModules([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 0x40) { throw "$Path is too small to be a PE file" }
    if ($bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { throw "$Path is not a PE file (no MZ)" }

    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    if ([BitConverter]::ToUInt32($bytes, $peOffset) -ne 0x00004550) {
        throw "$Path is not a PE file (no PE\0\0 at $peOffset)"
    }

    $coff = $peOffset + 4
    $sectionCount = [BitConverter]::ToUInt16($bytes, $coff + 2)
    $optionalSize = [BitConverter]::ToUInt16($bytes, $coff + 16)
    $optional = $coff + 20

    # PE32 keeps the data directories 16 bytes earlier than PE32+ does: the
    # four ImageBase/Reserved fields differ in width between the two.
    $magic = [BitConverter]::ToUInt16($bytes, $optional)
    switch ($magic) {
        0x10B { $dataDirs = $optional + 96 }   # PE32
        0x20B { $dataDirs = $optional + 112 }  # PE32+
        default { throw "$Path has an unknown optional header magic 0x$($magic.ToString('X'))" }
    }
    $dirCount = [BitConverter]::ToUInt32($bytes, $dataDirs - 4)

    $sections = @()
    $sectionTable = $optional + $optionalSize
    for ($i = 0; $i -lt $sectionCount; $i++) {
        $s = $sectionTable + ($i * 40)
        $sections += [pscustomobject]@{
            VirtualAddress = [BitConverter]::ToUInt32($bytes, $s + 12)
            VirtualSize    = [BitConverter]::ToUInt32($bytes, $s + 8)
            RawSize        = [BitConverter]::ToUInt32($bytes, $s + 16)
            RawAddress     = [BitConverter]::ToUInt32($bytes, $s + 20)
        }
    }

    # The import tables store addresses as RVAs; on disk we need file offsets.
    function ConvertTo-FileOffset([uint32]$rva) {
        foreach ($s in $sections) {
            $span = [Math]::Max($s.VirtualSize, $s.RawSize)
            if ($rva -ge $s.VirtualAddress -and $rva -lt ($s.VirtualAddress + $span)) {
                return [int]($s.RawAddress + ($rva - $s.VirtualAddress))
            }
        }
        return -1
    }

    function Read-AsciiAt([int]$offset) {
        if ($offset -lt 0 -or $offset -ge $bytes.Length) { return $null }
        $end = $offset
        while ($end -lt $bytes.Length -and $bytes[$end] -ne 0) { $end++ }
        return [System.Text.Encoding]::ASCII.GetString($bytes, $offset, $end - $offset)
    }

    $modules = New-Object System.Collections.Generic.List[string]

    # Directory 1 is the import table, directory 13 the delay-load table. A
    # delay-loaded VCRUNTIME140 fails at first call rather than at load, which
    # is worse to diagnose, not better — so both are checked.
    #   import descriptor: Name RVA at +12, 20-byte entries, zero-terminated
    #   delay descriptor:  Name RVA at +4,  32-byte entries, zero-terminated
    $tables = @(
        [pscustomobject]@{ Index = 1;  Stride = 20; NameAt = 12 },
        [pscustomobject]@{ Index = 13; Stride = 32; NameAt = 4  }
    )
    foreach ($table in $tables) {
        if ($dirCount -le $table.Index) { continue }
        $rva = [BitConverter]::ToUInt32($bytes, $dataDirs + ($table.Index * 8))
        if ($rva -eq 0) { continue }
        $cursor = ConvertTo-FileOffset $rva
        if ($cursor -lt 0) { continue }
        while ($true) {
            if ($cursor + $table.Stride -gt $bytes.Length) { break }
            $empty = $true
            for ($b = 0; $b -lt $table.Stride; $b++) {
                if ($bytes[$cursor + $b] -ne 0) { $empty = $false; break }
            }
            if ($empty) { break }
            $nameRva = [BitConverter]::ToUInt32($bytes, $cursor + $table.NameAt)
            # A bound delay-load descriptor can store a VA rather than an RVA;
            # such an entry simply will not map, and is skipped.
            $name = Read-AsciiAt (ConvertTo-FileOffset $nameRva)
            if ($name) { $modules.Add($name) }
            $cursor += $table.Stride
        }
    }
    return $modules
}

if ($args.Count -eq 0) { throw "usage: assert-no-vcruntime.ps1 <path> [<path> ...]" }

$targets = New-Object System.Collections.Generic.List[string]
foreach ($arg in $args) {
    if (-not (Test-Path -LiteralPath $arg)) { throw "no such path: $arg" }
    if (Test-Path -LiteralPath $arg -PathType Container) {
        # Filtered with Where-Object rather than -Include: -Include is silently
        # ignored alongside -LiteralPath, which would hand the PE reader the
        # marker files and licence text sitting in the same directory.
        Get-ChildItem -LiteralPath $arg -Recurse -File |
            Where-Object { $_.Extension -in '.exe', '.dll' } |
            ForEach-Object { $targets.Add($_.FullName) }
    } else {
        $targets.Add((Resolve-Path -LiteralPath $arg).Path)
    }
}

$scanned = 0
$failures = New-Object System.Collections.Generic.List[string]
foreach ($target in $targets) {
    $leaf = Split-Path -Leaf $target
    if ($Skip -contains $leaf) {
        Write-Output "skip $leaf (Microsoft's prebuilt redistributable ConPTY)"
        continue
    }
    $scanned++
    $imports = Get-PEImportedModules $target
    $bad = @($imports | Where-Object {
        $Forbidden -contains [System.IO.Path]::GetFileNameWithoutExtension($_).ToLowerInvariant()
    })
    if ($bad.Count -gt 0) {
        $failures.Add("$leaf imports the Visual C++ redistributable: $($bad -join ', ')")
    } else {
        Write-Output "ok   $leaf ($($imports.Count) imported modules, no VC++ redistributable)"
    }
}

# An empty scan must not pass: a mistyped path would otherwise report success
# without having looked at anything.
if ($scanned -eq 0) { throw "assert-no-vcruntime.ps1 found no PE files to check in: $($args -join ', ')" }

if ($failures.Count -gt 0) {
    foreach ($failure in $failures) { Write-Output "::error::$failure" }
    throw ("these binaries need the Visual C++ Redistributable and will not start " +
           "without it (#902); check that .cargo/config.toml's +crt-static still " +
           "applies and that nothing set RUSTFLAGS for this build " +
           "($($failures.Count) binary/binaries)")
}

Write-Output "No Visual C++ redistributable imports in $scanned binary/binaries."

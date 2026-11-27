# Captures process counters for a single phase of an isolated Windows benchmark.
# Select PIDs from the fixture launch handles. No process names, command lines or paths are recorded.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [hashtable] $Processes,
    [Parameter(Mandatory)] [ValidateNotNullOrEmpty()] [string] $Phase,
    [ValidateRange(1, 86400)] [int] $Seconds = 30,
    [ValidateRange(250, 60000)] [int] $IntervalMs = 1000
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Windows process and GPU counters require PowerShell 7 on Windows.' }
if ($Processes.Count -eq 0) { throw 'Provide at least one role and its fixture-owned PID.' }
$targets = [ordered]@{}
$processIds = [Collections.Generic.HashSet[int]]::new()
try {
    foreach ($role in ($Processes.Keys | Sort-Object)) {
        $processId = [int]$Processes[$role]
        if ($processId -lt 1 -or -not $processIds.Add($processId)) { throw "Invalid or repeated PID for $role" }
        $process = Get-Process -Id $processId
        $targets[$role] = $process
        # Retain this instance's handle even if a PID gets reused during the run.
        $null = $process.Handle
    }
    $gpuAvailable = $true
    $gpuQuery = ($processIds | ForEach-Object { "Name LIKE 'pid[_]$($_)[_]%'" }) -join ' OR '
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $deadline = [int64]$Seconds * 1000
    do {
        $finalSample = $clock.ElapsedMilliseconds -ge $deadline
        $rows = foreach ($role in $targets.Keys) {
            $process = $targets[$role]
            $process.Refresh()
            if ($process.HasExited) { throw "Fixture process $role (PID $($process.Id)) exited during phase $Phase" }
            [ordered]@{
                role = $role; pid = $process.Id; phase = $Phase
                qpcMs = [Diagnostics.Stopwatch]::GetTimestamp() * (1000.0 / [Diagnostics.Stopwatch]::Frequency)
                cpuMs = $process.TotalProcessorTime.TotalMilliseconds
                workingSetBytes = $process.WorkingSet64; privateBytes = $process.PrivateMemorySize64
                gpuBusiestEnginePct = $null; gpuStatus = 'no-instance'
            }
        }
        if ($gpuAvailable) {
            try {
                $engines = Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -Filter $gpuQuery -Property Name,UtilizationPercentage
                foreach ($row in $rows) {
                    foreach ($engine in $engines) {
                        if ($engine.Name.StartsWith("pid_$($row.pid)_", [StringComparison]::OrdinalIgnoreCase) -and $null -ne $engine.UtilizationPercentage) {
                            $row.gpuStatus = 'sampled'
                            if ($null -eq $row.gpuBusiestEnginePct -or $engine.UtilizationPercentage -gt $row.gpuBusiestEnginePct) {
                                $row.gpuBusiestEnginePct = $engine.UtilizationPercentage
                            }
                        }
                    }
                }
            } catch {
                $gpuAvailable = $false
                Write-Warning "GPU counters unavailable: $($_.Exception.Message)"
            }
        }
        foreach ($row in $rows) {
            if (-not $gpuAvailable) { $row.gpuStatus = 'counter-unavailable' }
            [pscustomobject]$row
        }
        if (-not $finalSample) {
            $elapsed = $clock.ElapsedMilliseconds
            $next = [Math]::Min($deadline, ([long][Math]::Floor($elapsed / $IntervalMs) + 1) * $IntervalMs)
            if ($next -gt $elapsed) { Start-Sleep -Milliseconds ($next - $elapsed) }
        }
    } until ($finalSample)
} finally {
    foreach ($process in $targets.Values) { $process.Dispose() }
}

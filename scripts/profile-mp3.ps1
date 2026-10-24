# Run only after building tests/mp3_output_alloc.rs in release mode.
# A separate process per case isolates Windows process-memory high-water marks.
param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$OutputPath,
    [switch]$Untracked
)
$ErrorActionPreference = 'Stop'
$cases = @(
    @{ Kind='silence48k128'; Seconds=60; Tagged='false'; Runs=3 },
    @{ Kind='silence48k128'; Seconds=257; Tagged='false'; Runs=1 },
    @{ Kind='silence48k128'; Seconds=60; Tagged='true'; Runs=3 },
    @{ Kind='lame_sine44k_vbr'; Seconds=15; Tagged='true'; Runs=3 }
)
$results = @()
foreach ($case in $cases) {
    $info = [Diagnostics.ProcessStartInfo]::new((Resolve-Path -LiteralPath $Executable).Path)
    $info.Arguments = 'profile_whole_mp3 --ignored --nocapture --test-threads=1'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.Environment['AUDIOBOX_PROFILE_RUNS'] = [string]$case.Runs
    $info.Environment['AUDIOBOX_PROFILE_KIND'] = $case.Kind
    $info.Environment['AUDIOBOX_PROFILE_SECONDS'] = [string]$case.Seconds
    $info.Environment['AUDIOBOX_PROFILE_TAGGED'] = $case.Tagged
    if ($Untracked) { $info.Environment['AUDIOBOX_PROFILE_UNTRACKED'] = '1' }
    else { $info.Environment.Remove('AUDIOBOX_PROFILE_UNTRACKED') | Out-Null }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $info
    $process.Start() | Out-Null
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $peakWorkingSet = 0L
    while (-not $process.WaitForExit(20)) {
        $process.Refresh()
        $peakWorkingSet = [Math]::Max($peakWorkingSet, $process.PeakWorkingSet64)
    }
    $output = $stdout.GetAwaiter().GetResult()
    $errorOutput = $stderr.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) { throw "Profile failed: $output $errorOutput" }
    $result = [PSCustomObject]@{
        Kind=$case.Kind; Seconds=$case.Seconds; Tagged=$case.Tagged; Runs=$case.Runs
        Instrumented=(-not $Untracked); PeakWorkingSetBytes=$peakWorkingSet
        ProcessCpuSeconds=$process.TotalProcessorTime.TotalSeconds
        Output=$output
    }
    $results += $result
    $results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath
    $result | Select-Object Kind,Seconds,Tagged,Runs,Instrumented,PeakWorkingSetBytes,ProcessCpuSeconds
    $process.Dispose()
}

# Run with NVIDIA Instant Replay off. Uses only an unused drive letter.
param([char]$Drive = 'T')
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
if ($Drive -cnotmatch '^[D-Z]$') { throw 'Use an unused letter from D through Z.' }
$mount = "${Drive}:"
$root = "${Drive}:\"
if (Test-Path $root) { throw "$mount is already in use." }
$keyPath = 'Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS'
$hive = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', 'Registry64')
$key = $hive.OpenSubKey($keyPath, $true)
if (!$key) { throw 'Configure NVIDIA overlay temporary files first.' }
$original = $key.GetValue('TempFilePath', $null, 'DoNotExpandEnvironmentNames')
if ($null -eq $original) { throw 'NVIDIA TempFilePath is absent.' }
$kind = $key.GetValueKind('TempFilePath')
$target = "${Drive}:\NVIDIA-Replay"
$info = [System.Diagnostics.ProcessStartInfo]::new()
$info.FileName = "$PWD\dist\nvidia-mem-replay.exe"
$info.Arguments = "filesystem --drive $Drive --memory-limit-mb 256"
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.RedirectStandardInput = $true
$info.RedirectStandardOutput = $true
$process = [System.Diagnostics.Process]::Start($info)
function Read-Sample {
    $task = $process.StandardOutput.ReadLineAsync()
    if (!$task.Wait(5000)) { throw 'Helper telemetry timed out.' }
    if (!$task.Result) { throw 'Helper exited without telemetry.' }
    return ($task.Result | ConvertFrom-Json)
}
try {
    $baseline = Read-Sample
    if ($baseline.version -ne 1) { throw 'Wrong telemetry version.' }
    New-Item -ItemType Directory -Path $target | Out-Null
    $replacement = if ($kind -eq 'Binary') { [Text.Encoding]::Unicode.GetBytes($target + [char]0) } else { $target }
    $key.SetValue('TempFilePath', $replacement, $kind)
    $key.Flush()
    $file = "$target\counter.bin"
    $stream = [IO.FileStream]::new($file, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough)
    try {
        $bytes = [byte[]]::new(1000000)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
        $stream.Position = 0
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do { $sample = Read-Sample } while ($sample.written_bytes -lt 2000000 -and [DateTime]::UtcNow -lt $deadline)
    if ($sample.written_bytes -ne 2000000) { throw "Overwrite accounting failed: $($sample.written_bytes)" }
    Remove-Item $file
    $deleted = Read-Sample
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ($deleted.buffer_bytes -ne 0 -and [DateTime]::UtcNow -lt $deadline) { $deleted = Read-Sample }
    if ($deleted.buffer_bytes -ne 0) { throw 'Deleted file memory was not released.' }
    if ($deleted.written_bytes -lt 2000000) { throw 'Deletion reduced lifetime writes.' }
    $capacityFailed = $false
    try {
        $oversized = [IO.FileStream]::new("$target\too-big.bin", [IO.FileMode]::Create, [IO.FileAccess]::Write)
        try { $oversized.SetLength(257000000) } finally { $oversized.Dispose() }
    } catch [IO.IOException] { $capacityFailed = $true }
    if (!$capacityFailed) { throw 'Filesystem did not enforce its memory ceiling.' }
    # EOF models owner exit/crash, without sending the orderly stop command.
    $process.StandardInput.Close()
    if (!$process.WaitForExit(5000)) { throw 'Helper did not stop after owner exit.' }
    if ($process.ExitCode -ne 0) { throw "Helper exited with code $($process.ExitCode)." }
    $restored = $key.GetValue('TempFilePath', $null, 'DoNotExpandEnvironmentNames')
    if ($kind -eq 'Binary') {
        if ([Convert]::ToBase64String($restored) -ne [Convert]::ToBase64String($original)) { throw 'Original registry bytes were not restored.' }
    } elseif ($restored -cne $original) { throw 'Original temporary path was not restored.' }
    if (Test-Path $root) { throw 'RAM volume was not unmounted.' }
    Write-Host 'PASS: mount, writes, overwrite, deletion, capacity, owner exit, restoration.'
} finally {
    if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
    # Test cleanup preserves later edits just like the application.
    $current = $key.GetValue('TempFilePath', $null, 'DoNotExpandEnvironmentNames')
    $ours = if ($kind -eq 'Binary') { [Text.Encoding]::Unicode.GetString($current).TrimEnd([char]0) -ceq $target } else { $current -ceq $target }
    if ($ours) { $key.SetValue('TempFilePath', $original, $kind); $key.Flush() }
    $key.Dispose()
    $hive.Dispose()
    $process.Dispose()
}

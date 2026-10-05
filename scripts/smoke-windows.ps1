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
    # Share the authoritative recovery snapshot before waiting for readiness.
    $originalBytes = if ($kind -eq 'Binary') { [byte[]]$original } else { [Text.Encoding]::Unicode.GetBytes($original + [char]0) }
    $replacementBytes = [Text.Encoding]::Unicode.GetBytes($target + [char]0)
    $snapshot = @{
        original = @{ kind = [uint32]$kind; bytes = @($originalBytes | ForEach-Object { [int]$_ }) }
        replacement = @{ kind = [uint32]$kind; bytes = @($replacementBytes | ForEach-Object { [int]$_ }) }
        original_path = [Text.Encoding]::Unicode.GetString($originalBytes).TrimEnd([char]0)
        target = $target
    }
    $process.StandardInput.WriteLine(($snapshot | ConvertTo-Json -Depth 4 -Compress))
    $process.StandardInput.Flush()
    $baseline = Read-Sample
    if ($baseline.version -ne 1) { throw 'Wrong telemetry version.' }
    if (!(Test-Path -LiteralPath $target -PathType Container)) { throw 'Helper reported readiness without creating its recording directory.' }
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
    # Reusing an allocated sector must not expose bytes discarded by truncation.
    $truncatedPath = "$target\truncate.bin"
    $stream = [IO.FileStream]::new($truncatedPath, [IO.FileMode]::Create, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $pattern = [byte[]]::new(65536)
        for ($i = 0; $i -lt $pattern.Length; $i++) { $pattern[$i] = 65 }
        $stream.Write($pattern, 0, $pattern.Length)
        $stream.Flush($true)
        $stream.SetLength(4096)
        $stream.SetLength(8192)
        $stream.Position = 4096
        $tail = [byte[]]::new(4096)
        $read = 0
        while ($read -lt $tail.Length) {
            $count = $stream.Read($tail, $read, $tail.Length - $read)
            if ($count -eq 0) { throw 'Truncate/extend returned an incomplete file.' }
            $read += $count
        }
        if ($tail | Where-Object { $_ -ne 0 } | Select-Object -First 1) { throw 'Truncate/extend exposed old file bytes.' }
    } finally { $stream.Dispose() }
    Remove-Item -LiteralPath $truncatedPath
    # Replace through the Windows rename API with colliding alternate stream names.
    if (!("ReplaySmoke.Native" -as [type])) {
        Add-Type @'
using System.Runtime.InteropServices;
namespace ReplaySmoke {
    public static class Native {
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool MoveFileExW(string source, string destination, uint flags);
    }
}
'@
    }
    $renameSource = "$target\rename-source.bin"
    $renameTarget = "$target\rename-target.bin"
    [IO.File]::WriteAllText($renameSource, 'source file')
    [IO.File]::WriteAllText("${renameSource}:meta", 'source stream')
    [IO.File]::WriteAllText($renameTarget, 'old destination file')
    [IO.File]::WriteAllText("${renameTarget}:meta", 'old destination stream')
    [IO.File]::WriteAllText("${renameTarget}:obsolete", 'destination-only stream')
    if (![ReplaySmoke.Native]::MoveFileExW($renameSource, $renameTarget, 1)) {
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
    }
    if ([IO.File]::ReadAllText($renameTarget) -cne 'source file' -or
        [IO.File]::ReadAllText("${renameTarget}:meta") -cne 'source stream' -or
        [IO.File]::Exists("${renameTarget}:obsolete") -or [IO.File]::Exists($renameSource)) {
        throw 'Rename did not replace the file and its alternate streams together.'
    }
    Remove-Item -LiteralPath $renameTarget
    $capacityFailed = $false
    try {
        $oversized = [IO.FileStream]::new("$target\too-big.bin", [IO.FileMode]::Create, [IO.FileAccess]::Write)
        try { $oversized.SetLength(257000000) } finally { $oversized.Dispose() }
    } catch [IO.IOException] { $capacityFailed = $true }
    if (!$capacityFailed) { throw 'Filesystem did not enforce its memory ceiling.' }
    # EOF models owner exit/crash, without sending the orderly stop command.
    $process.StandardInput.Close()
    # Keep draining telemetry while the supervisor publishes its final counter.
    $remainingOutput = $process.StandardOutput.ReadToEndAsync()
    if (!$process.WaitForExit(10000)) { throw 'Helper did not stop after owner exit.' }
    if (!$remainingOutput.Wait(1000)) { throw 'Helper telemetry did not close after exit.' }
    if ($process.ExitCode -ne 0) { throw "Helper exited with code $($process.ExitCode)." }
    $restored = $key.GetValue('TempFilePath', $null, 'DoNotExpandEnvironmentNames')
    if ($kind -eq 'Binary') {
        if ([Convert]::ToBase64String($restored) -ne [Convert]::ToBase64String($original)) { throw 'Original registry bytes were not restored.' }
    } elseif ($restored -cne $original) { throw 'Original temporary path was not restored.' }
    if (Test-Path $root) { throw 'RAM volume was not unmounted.' }
    Write-Host 'PASS: mount, writes, overwrite, deletion, truncate/extend, stream replacement, capacity, owner exit, restoration.'
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

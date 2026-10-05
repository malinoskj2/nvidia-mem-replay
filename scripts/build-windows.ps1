param([switch]$SkipInstaller)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
. "$PSScriptRoot/prepare-winfsp.ps1"
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (!(Test-Path $vswhere)) { throw 'Install Visual Studio C++ build tools.' }
$msbuild = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
if (!$msbuild) { throw 'Visual Studio C++ MSBuild was not found.' }
$nativeOutput = Join-Path $PWD 'target\memefs\'
& $msbuild vendor\memefs\WinFsp-MemFs-Extended.vcxproj /m /t:Rebuild /p:Configuration=Release /p:Platform=x64 "/p:WinFspSdk=$winfspSdk" "/p:OutDir=$nativeOutput" "/p:IntDir=$nativeOutput\obj\"
if ($LASTEXITCODE -ne 0) { throw 'MemFS Extended build failed' }
cargo build --locked --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
# Replace generated packages so obsolete filesystem runtimes cannot ship.
if (Test-Path dist) { Remove-Item dist -Recurse -Force }
New-Item -ItemType Directory -Force dist\licenses, dist\source | Out-Null
Copy-Item target\x86_64-pc-windows-msvc\release\nvidia-mem-replay.exe dist\
Copy-Item "$nativeOutput\memefs-x64.exe" dist\
Copy-Item $setup dist\winfsp-2.1.25156.msi
Copy-Item $source dist\source\winfsp-2.1-source.tar.gz
Copy-Item README.md, LICENSE dist\
Copy-Item docs\verification.md dist\
Copy-Item licenses\* dist\licenses\
# Include complete dependency sources and notices with the GPL application source.
$stage = Join-Path $cache 'replay-source'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
$sourceFiles = @('src', 'vendor', 'scripts', 'installer', 'licenses', 'docs', '.cargo', 'Cargo.toml', 'Cargo.lock', 'LICENSE', 'README.md')
foreach ($item in $sourceFiles) { Copy-Item $item $stage -Recurse }
$dependencies = Join-Path $stage 'dependencies'
$vendorConfig = cargo vendor --locked --versioned-dirs $dependencies
if ($LASTEXITCODE -ne 0) { throw 'Dependency source bundling failed' }
$portableConfig = [regex]::Replace(($vendorConfig -join "`n"), '(?m)^directory = .+$', 'directory = "dependencies"')
Add-Content -Path "$stage/.cargo/config.toml" -Value "`n$portableConfig" -Encoding utf8
Compress-Archive -Path "$stage/*", "$stage/.cargo" -DestinationPath dist/source/replay-in-ram-source.zip -Force
if (!$SkipInstaller) {
    $nsis = Get-Command makensis -ErrorAction SilentlyContinue
    if (!$nsis) {
        $candidate = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
        if (!(Test-Path $candidate)) { throw 'Install NSIS 3 to produce the installer, or pass -SkipInstaller.' }
        $compiler = $candidate
    } else { $compiler = $nsis.Source }
    & $compiler /WX installer\replay.nsi
    if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
}
Write-Host 'Built Windows x64 application and WinFsp/MemFS Extended bundle in dist.'

Get-ChildItem dist -File -Recurse | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object FullName | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath((Join-Path $PWD 'dist'), $_.FullName).Replace('\', '/')
    "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $relative
} | Set-Content dist/SHA256SUMS.txt -Encoding utf8

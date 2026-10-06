$ErrorActionPreference = 'Stop'
$cache = Join-Path $PWD 'target\winfsp-package'
New-Item -ItemType Directory -Force $cache | Out-Null

function Fetch-Verified($name, $url, $sha256) {
    $path = Join-Path $cache $name
    if (!(Test-Path $path) -or (Get-FileHash $path -Algorithm SHA256).Hash -ne $sha256) {
        Invoke-WebRequest -Uri $url -OutFile $path
    }

    if ((Get-FileHash $path -Algorithm SHA256).Hash -ne $sha256) { throw "SHA-256 mismatch: $name" }

    return $path
}

$setup = Fetch-Verified 'winfsp-2.1.25156.msi' 'https://github.com/winfsp/winfsp/releases/download/v2.1/winfsp-2.1.25156.msi' '073a70e00f77423e34bed98b86e600def93393ba5822204fac57a29324db9f7a'
$source = Fetch-Verified 'winfsp-2.1-source.tar.gz' 'https://github.com/winfsp/winfsp/archive/refs/tags/v2.1.tar.gz' '90362446cd31dee56cec0931716ebc12def6abf1307f43d68d7677f1e3ddaad9'

$extracted = Join-Path $cache 'sdk'
$sdk = Get-ChildItem $extracted -Recurse -Filter winfsp-x64.lib -ErrorAction SilentlyContinue | Where-Object { $_.Directory.Name -eq 'lib' -and (Test-Path (Join-Path $_.Directory.Parent.FullName 'inc\winfsp\winfsp.h')) } | Select-Object -First 1
if (!$sdk) {
    New-Item -ItemType Directory -Force $extracted | Out-Null
    # Administrative extraction supplies the SDK without installing a build-host driver.
    $process = Start-Process msiexec.exe -ArgumentList @('/a', "`"$setup`"", '/qn', "TARGETDIR=`"$extracted`"") -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "WinFsp SDK extraction failed: $($process.ExitCode)" }
    $sdk = Get-ChildItem $extracted -Recurse -Filter winfsp-x64.lib | Where-Object { Test-Path (Join-Path $_.Directory.Parent.FullName 'inc\winfsp\winfsp.h') } | Select-Object -First 1
}

if (!$sdk) { throw 'Extracted WinFsp SDK is missing headers or import library.' }
$winfspSdk = $sdk.Directory.Parent.FullName

# Install a verified Mitos release for the current Windows user.
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Mitos'),
    [switch]$NoModifyPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
    throw 'Use install.sh on Linux or macOS.'
}
$architecture = $env:PROCESSOR_ARCHITEW6432
if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
switch ($architecture) {
    'AMD64' { $arch = 'x86_64' }
    'ARM64' { $arch = 'aarch64' }
    default { throw "Unsupported Windows architecture: $architecture" }
}
# Windows PowerShell 5.1 may otherwise negotiate an obsolete TLS version.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
if ($Version -eq 'latest') {
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/mitos-editor/mitos/releases/latest'
    $Version = $release.tag_name
}
$Version = $Version -replace '^v', ''
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Expected a stable release version such as 0.1.0, got: $Version"
}
$tag = "v$Version"
$package = "mitos-$tag-$arch-windows"
$archive = "$package.zip"
$releaseUrl = "https://github.com/mitos-editor/mitos/releases/download/$tag"
$workDir = Join-Path ([IO.Path]::GetTempPath()) ("mitos-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $workDir | Out-Null
try {
    Write-Host "Downloading Mitos $Version for Windows/$arch..."
    $archivePath = Join-Path $workDir $archive
    Invoke-WebRequest -UseBasicParsing -Uri "$releaseUrl/$archive" -OutFile $archivePath
    $checksumUrl = "$releaseUrl/SHA256SUMS"
    if ($tag -eq 'v0.1.0') { $checksumUrl = "https://mitos.computer/checksums/$tag.txt" }
    $checksumPath = Join-Path $workDir 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing -Uri $checksumUrl -OutFile $checksumPath
    $pattern = '^([0-9a-fA-F]{64})\s+' + [regex]::Escape($archive) + '$'
    $checksums = @(Get-Content -LiteralPath $checksumPath | Select-String -Pattern $pattern)
    if ($checksums.Count -ne 1) { throw "No unique SHA-256 checksum found for $archive" }
    $expected = $checksums[0].Matches[0].Groups[1].Value
    $actual = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
    if ($actual -ne $expected) { throw 'Checksum mismatch; nothing was installed.' }
    Expand-Archive -LiteralPath $archivePath -DestinationPath $workDir
    $extracted = Join-Path $workDir $package
    if (-not (Test-Path -LiteralPath (Join-Path $extracted 'ms.exe') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $extracted 'runtime') -PathType Container)) {
        throw 'The archive does not contain an executable and its runtime.'
    }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
    $binDir = Join-Path $InstallDir 'bin'
    $versionsDir = Join-Path $InstallDir 'versions'
    $destination = Join-Path $versionsDir "$tag-$arch-windows"
    New-Item -ItemType Directory -Force -Path $binDir, $versionsDir | Out-Null
    $launcher = Join-Path $binDir 'ms.cmd'
    if (Test-Path -LiteralPath (Join-Path $binDir 'ms.exe')) {
        throw "$binDir\ms.exe already exists. Choose another -InstallDir."
    }
    if ((Test-Path -LiteralPath $launcher) -and
        (Get-Content -LiteralPath $launcher -First 1) -ne ':: Mitos installer') {
        throw "$launcher belongs to another installation. Choose another -InstallDir."
    }
    if (Test-Path -LiteralPath $destination) {
        if (-not (Test-Path -LiteralPath (Join-Path $destination 'ms.exe') -PathType Leaf) -or
            -not (Test-Path -LiteralPath (Join-Path $destination 'runtime') -PathType Container)) {
            throw "Incomplete installation already exists at $destination"
        }
    } else {
        Move-Item -LiteralPath $extracted -Destination $destination
    }
    # A relative cmd launcher needs no symlink privilege and preserves runtime lookup.
    $contents = ":: Mitos installer`r`n@echo off`r`n`"%~dp0..\versions\$tag-$arch-windows\ms.exe`" %*`r`n"
    $temporaryLauncher = Join-Path $binDir ('.ms-install-' + [guid]::NewGuid() + '.cmd')
    Set-Content -LiteralPath $temporaryLauncher -Value $contents -Encoding ASCII
    Move-Item -LiteralPath $temporaryLauncher -Destination $launcher -Force
    if (-not $NoModifyPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $entries = @($userPath -split ';' | Where-Object { $_ })
        if ($entries -notcontains $binDir) {
            [Environment]::SetEnvironmentVariable('Path', (($entries + $binDir) -join ';'), 'User')
        }
        if (@($env:Path -split ';') -notcontains $binDir) { $env:Path = "$binDir;$env:Path" }
    }
    Write-Host "Installed Mitos $Version. Run: ms --health"
    if ($NoModifyPath) { Write-Host "Add this directory to your PATH: $binDir" }
} finally {
    Remove-Item -LiteralPath $workDir -Recurse -Force
}

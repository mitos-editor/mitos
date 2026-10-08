# Run the installer against local releases without modifying the user's PATH.
$ErrorActionPreference = 'Stop'
$env:OS = 'Windows_NT'
$env:PROCESSOR_ARCHITECTURE = $env:FIXTURE_ARCH
$env:PROCESSOR_ARCHITEW6432 = ''

function Invoke-RestMethod {
    param($Uri)
    @{ tag_name = $env:FIXTURE_TAG }
}

function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing)
    $name = ($Uri -split '/')[-1]
    if ($name -eq 'SHA256SUMS') {
        $name = ($Uri -split '/')[-2] + '.txt'
    }
    Copy-Item -LiteralPath (Join-Path $env:FIXTURE_DIR $name) -Destination $OutFile
}

if ($env:FIXTURE_PS_VERSION) {
    $PSVersionTable.PSVersion = [version]$env:FIXTURE_PS_VERSION
    $script = [scriptblock]::Create((Get-Content -LiteralPath $env:INSTALLER -Raw))
    & $script -InstallDir $env:INSTALL_DIR -NoModifyPath @args
} else {
    & $env:INSTALLER -InstallDir $env:INSTALL_DIR -NoModifyPath @args
}

[CmdletBinding()]
param(
    [string]$Version = '1.1.2.0',
    [string]$Publisher = 'CN=ZWG Terminal Test',
    [ValidateSet('x64', 'arm64')]
    [string]$Architecture = 'x64',
    [string]$Configuration = 'release',
    [string]$PfxPath = '',
    [string]$PfxPassword = '',
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$stage = Join-Path $PSScriptRoot '_msix_stage'
$output = Join-Path $PSScriptRoot 'dist'
$manifestTemplate = Join-Path $PSScriptRoot 'AppxManifest.xml'
$exePath = Join-Path $root ('target\' + $Configuration + '\zwg.exe')
$resourcesPath = Join-Path $root 'resources'

if (-not (Test-Path $exePath)) {
    throw ('zwg.exe not found. Run cargo build --' + $Configuration + ' first.')
}

function Resolve-Tool([string]$toolName) {
    $command = Get-Command $toolName -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $programFilesX86 = [System.Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10\bin'
    if (-not (Test-Path $kitsRoot)) {
        throw ($toolName + ' not found. Install the Windows 10/11 SDK.')
    }

    $candidate = Get-ChildItem $kitsRoot -Directory |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName ('x64\' + $toolName) } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1

    if (-not $candidate) {
        throw ($toolName + ' not found under Windows Kits.')
    }

    return $candidate
}

$makeappx = Resolve-Tool 'makeappx.exe'
$signtool = Resolve-Tool 'signtool.exe'

Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $stage -ItemType Directory | Out-Null
New-Item $output -ItemType Directory -Force | Out-Null

Copy-Item $exePath (Join-Path $stage 'zwg.exe')
Copy-Item $resourcesPath (Join-Path $stage 'resources') -Recurse

$manifest = Get-Content $manifestTemplate -Raw
$manifest = $manifest.Replace('__VERSION__', $Version)
$manifest = $manifest.Replace('__PUBLISHER__', $Publisher)
$manifest = $manifest.Replace('__ARCH__', $Architecture)
$manifest | Set-Content (Join-Path $stage 'AppxManifest.xml') -Encoding UTF8

$packagePath = Join-Path $output ('ZWG_Terminal_' + $Version + '_' + $Architecture + '.msix')
if (Test-Path $packagePath) {
    Remove-Item $packagePath -Force
}

& $makeappx pack /v /h SHA256 /d $stage /p $packagePath
if ($LASTEXITCODE -ne 0) {
    throw ('makeappx failed with exit code ' + $LASTEXITCODE)
}

if ($PfxPath) {
    $signArgs = @(
        'sign',
        '/fd', 'SHA256',
        '/f', $PfxPath,
        '/tr', $TimestampUrl,
        '/td', 'SHA256'
    )
    if ($PfxPassword) {
        $signArgs += @('/p', $PfxPassword)
    }
    $signArgs += $packagePath
    & $signtool @signArgs
    if ($LASTEXITCODE -ne 0) {
        throw ('signtool failed with exit code ' + $LASTEXITCODE)
    }
}

Write-Host ('MSIX package created: ' + $packagePath)

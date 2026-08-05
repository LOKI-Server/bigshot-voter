# Build a release binary and package a Windows installer (or zip fallback).
#
# Usage (from repo root or this directory):
#   .\packaging\windows\build-installer.ps1
#   .\packaging\windows\build-installer.ps1 -Profile release-size
#   .\packaging\windows\build-installer.ps1 -SkipBuild
#
# Requires: Rust toolchain. For a Setup .exe also needs Inno Setup 6
#   (https://jrsoftware.org/isinfo.php) or winget install JRSoftware.InnoSetup

[CmdletBinding()]
param(
    [ValidateSet("release", "release-size")]
    [string]$Profile = "release",

    [switch]$SkipBuild,

    # Skip Inno and only produce a portable zip under target\windows-installer
    [switch]$ZipOnly
)

$ErrorActionPreference = "Stop"

function Get-RepoRoot {
    $here = $PSScriptRoot
    if (-not $here) { $here = Split-Path -Parent $MyInvocation.MyCommand.Path }
    return (Resolve-Path (Join-Path $here "..\..")).Path
}

function Get-CargoVersion([string]$CargoToml) {
    $text = Get-Content -Raw -Path $CargoToml
    if ($text -match '(?m)^version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }
    throw "Could not read version from $CargoToml"
}

function Find-ISCC {
    $candidates = @(
        "${env:LocalAppData}\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path -LiteralPath $c)) { return $c }
    }
    $cmd = Get-Command iscc -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function New-PortableZip {
    param(
        [string]$ExePath,
        [string]$PackagingDir,
        [string]$OutDir,
        [string]$Version
    )

    $stage = Join-Path $OutDir "portable-stage"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item -LiteralPath $ExePath -Destination (Join-Path $stage "bigshot-voter.exe")
    Copy-Item -LiteralPath (Join-Path $PackagingDir "launch.cmd") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $PackagingDir "README-install.txt") `
        -Destination (Join-Path $stage "README.txt")

    $zipName = "BigShotVoting-$Version-portable-win64.zip"
    $zipPath = Join-Path $OutDir $zipName
    if (Test-Path $zipPath) { Remove-Item -Force $zipPath }

    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zipPath -CompressionLevel Optimal
    Remove-Item -Recurse -Force $stage
    return $zipPath
}

$root = Get-RepoRoot
Set-Location $root

$cargoToml = Join-Path $root "Cargo.toml"
$version = Get-CargoVersion $cargoToml
$packagingDir = Join-Path $root "packaging\windows"
$outDir = Join-Path $root "target\windows-installer"
$exeRel = if ($Profile -eq "release-size") {
    "target\release-size\bigshot-voter.exe"
} else {
    "target\release\bigshot-voter.exe"
}
$exePath = Join-Path $root $exeRel

Write-Host "==> Big-Shot Voting Windows packaging v$version (profile: $Profile)" -ForegroundColor Cyan

if (-not $SkipBuild) {
    Write-Host "==> cargo build --profile $Profile" -ForegroundColor Cyan
    & cargo build --profile $Profile
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}

if (-not (Test-Path -LiteralPath $exePath)) {
    throw "Missing binary: $exePath (build first or drop -SkipBuild)"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$zipPath = New-PortableZip -ExePath $exePath -PackagingDir $packagingDir -OutDir $outDir -Version $version
Write-Host "==> Portable zip: $zipPath" -ForegroundColor Green

if ($ZipOnly) {
    Write-Host "Done (zip only)." -ForegroundColor Green
    exit 0
}

$iscc = Find-ISCC
if (-not $iscc) {
    Write-Host @"

Inno Setup 6 was not found — Setup .exe was skipped.
Install it, then re-run with -SkipBuild:

  winget install JRSoftware.InnoSetup
  .\packaging\windows\build-installer.ps1 -SkipBuild

Portable package is ready: $zipPath

Unzip to a writable folder (e.g. %LOCALAPPDATA%\Programs\bigshot-voter)
so the default data\ directory next to the exe works.
"@ -ForegroundColor Yellow
    exit 0
}

$iss = Join-Path $packagingDir "bigshot-voter.iss"
Write-Host "==> Inno Setup: $iscc" -ForegroundColor Cyan

# ISCC defines: paths relative to the .iss file location
$sourceExeForIss = (Resolve-Path $exePath).Path
$outDirForIss = (Resolve-Path $outDir).Path

& $iscc `
    "/DAppVersion=$version" `
    "/DSourceExe=$sourceExeForIss" `
    "/DOutputDir=$outDirForIss" `
    $iss

if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

$setup = Get-ChildItem -Path $outDir -Filter "BigShotVoting-*-setup.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if ($setup) {
    Write-Host "==> Installer: $($setup.FullName)" -ForegroundColor Green
    Write-Host "    Default install dir: %LOCALAPPDATA%\Programs\bigshot-voter (writable; data\ works)" -ForegroundColor DarkGray
} else {
    Write-Host "ISCC finished but setup exe not found under $outDir" -ForegroundColor Yellow
}

Write-Host "Done." -ForegroundColor Green

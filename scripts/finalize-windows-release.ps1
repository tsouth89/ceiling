#Requires -Version 5.1
<#
.SYNOPSIS
    Verify signed Windows release artifacts and write their SHA-256 sidecars.

.DESCRIPTION
    Fails unless the desktop app, CLI, portable app, and installer all have a
    valid Authenticode signature from the expected publisher. Hash sidecars are
    deliberately generated only after signature verification.
#>

param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$WorkRoot = "C:\code\Ceiling-release",

    [string]$ExpectedSigner = "CN=Brandon South"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$releaseBinDir = Join-Path $WorkRoot "cache\cargo-target\x86_64-pc-windows-msvc\release"
$assetsDir = Join-Path $WorkRoot "assets"
$desktopExe = Join-Path $releaseBinDir "ceiling.exe"
$cliExe = Join-Path $releaseBinDir "codexbar-cli.exe"
$installer = Join-Path $assetsDir "Ceiling-$Version-Setup.exe"
$storeInstaller = Join-Path $assetsDir "Ceiling-$Version-Store-Setup.exe"
$portable = Join-Path $assetsDir "Ceiling-$Version-portable.exe"

function Assert-ReleaseSignature {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Missing signed release file: $Path"
    }

    $signature = Get-AuthenticodeSignature -FilePath $Path
    if ($signature.Status -ne "Valid") {
        throw "Invalid Authenticode signature for $Path. Status: $($signature.Status)"
    }

    $subject = $signature.SignerCertificate.Subject
    if ($subject -notlike "*$ExpectedSigner*") {
        throw "Unexpected signer for $Path. Expected $ExpectedSigner, got $subject"
    }

    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing RFC 3161 timestamp for $Path"
    }

    Write-Host "[signed] $(Split-Path $Path -Leaf): $subject"
}

foreach ($path in @($desktopExe, $cliExe, $portable, $installer, $storeInstaller)) {
    Assert-ReleaseSignature -Path $path
}

foreach ($asset in @($installer, $storeInstaller, $portable)) {
    $fileName = Split-Path $asset -Leaf
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $asset).Hash.ToLowerInvariant()
    "$hash  $fileName" | Set-Content -Encoding ascii "$asset.sha256"
    Write-Host "[sha256] ${fileName}: $hash"
}

Write-Host "Signed Windows release artifacts finalized."

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$AssetsDir,

    [string]$BucketName = "ceiling-downloads",
    [string]$DownloadOrigin = "https://downloads.ceiling.win",
    [string]$WranglerVersion = "4.113.0",
    [switch]$SkipPublicVerification
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$resolvedAssetsDir = (Resolve-Path -LiteralPath $AssetsDir).Path
$installerName = "Ceiling-$Version-Setup.exe"
$hashName = "$installerName.sha256"
$installerPath = Join-Path $resolvedAssetsDir $installerName
$hashPath = Join-Path $resolvedAssetsDir $hashName

foreach ($path in @($installerPath, $hashPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required release asset not found: $path"
    }
}

$hashContents = Get-Content -LiteralPath $hashPath -Raw
if ($hashContents -notmatch '(?i)\b([0-9a-f]{64})\b') {
    throw "Could not read a SHA-256 value from $hashPath."
}
$expectedHash = $Matches[1].ToLowerInvariant()
$localHash = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($localHash -ne $expectedHash) {
    throw "Installer SHA-256 does not match its sidecar. Expected $expectedHash, got $localHash."
}

$objectPrefix = "releases/v$Version"

function Publish-R2Object {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$ContentType,

        [Parameter(Mandatory = $true)]
        [string]$CacheControl
    )

    $name = Split-Path -Leaf $Path
    $objectPath = "$BucketName/$objectPrefix/$name"
    & npx --yes "wrangler@$WranglerVersion" r2 object put $objectPath `
        "--file=$Path" `
        --remote `
        --force `
        "--content-type=$ContentType" `
        "--cache-control=$CacheControl"
    if ($LASTEXITCODE -ne 0) {
        throw "Wrangler failed to upload $name to R2."
    }
}

Publish-R2Object `
    -Path $installerPath `
    -ContentType "application/vnd.microsoft.portable-executable" `
    -CacheControl "public, max-age=31536000, immutable"
Publish-R2Object `
    -Path $hashPath `
    -ContentType "text/plain; charset=utf-8" `
    -CacheControl "public, max-age=31536000, immutable"

$installerUrl = "$($DownloadOrigin.TrimEnd('/'))/$objectPrefix/$installerName"
if ($SkipPublicVerification) {
    Write-Output "Uploaded Microsoft Store installer: $installerUrl"
    return
}

$downloadPath = Join-Path ([System.IO.Path]::GetTempPath()) "ceiling-store-$Version-$([guid]::NewGuid().ToString('N')).exe"
try {
    & curl.exe `
        --fail `
        --silent `
        --show-error `
        --location `
        --max-redirs 0 `
        --connect-timeout 10 `
        --max-time 180 `
        --retry 3 `
        --retry-delay 5 `
        --retry-connrefused `
        --output $downloadPath `
        $installerUrl
    if ($LASTEXITCODE -ne 0) {
        throw "Direct public download failed with curl exit code $LASTEXITCODE."
    }

    $downloadHash = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($downloadHash -ne $expectedHash) {
        throw "Public installer SHA-256 mismatch. Expected $expectedHash, got $downloadHash."
    }
} finally {
    Remove-Item -LiteralPath $downloadPath -Force -ErrorAction SilentlyContinue
}

Write-Output "Verified direct Microsoft Store installer URL: $installerUrl"

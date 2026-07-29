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
$installerName = "Ceiling-$Version-Store-Setup.exe"
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
$installerUrl = "$($DownloadOrigin.TrimEnd('/'))/$objectPrefix/$installerName"
$hashUrl = "$installerUrl.sha256"

function Get-PublicObject {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Url,

        [Parameter(Mandatory = $true)]
        [string]$Destination
    )

    $status = & curl.exe `
        --silent `
        --show-error `
        --location `
        --max-redirs 0 `
        --connect-timeout 10 `
        --max-time 180 `
        --retry 3 `
        --retry-delay 5 `
        --retry-connrefused `
        --output $Destination `
        --write-out "%{http_code}" `
        $Url
    if ($LASTEXITCODE -ne 0) {
        throw "Could not check existing immutable object $Url (curl exit $LASTEXITCODE)."
    }

    return [int]$status
}

$existingInstallerPath = Join-Path ([System.IO.Path]::GetTempPath()) "ceiling-store-existing-$Version-$([guid]::NewGuid().ToString('N')).exe"
$existingHashPath = "$existingInstallerPath.sha256"
$skipUpload = $false
try {
    $installerStatus = Get-PublicObject -Url $installerUrl -Destination $existingInstallerPath
    if ($installerStatus -eq 200) {
        $existingHash = (Get-FileHash -LiteralPath $existingInstallerPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -ne $expectedHash) {
            throw "Refusing to overwrite immutable release object $installerUrl. Existing SHA-256 is $existingHash; expected $expectedHash."
        }

        $hashStatus = Get-PublicObject -Url $hashUrl -Destination $existingHashPath
        if ($hashStatus -ne 200) {
            throw "Installer already exists, but its immutable checksum sidecar returned HTTP ${hashStatus}: $hashUrl"
        }
        $existingHashText = Get-Content -LiteralPath $existingHashPath -Raw
        if ($existingHashText -notmatch '(?i)\b([0-9a-f]{64})\b' -or
            $Matches[1].ToLowerInvariant() -ne $expectedHash) {
            throw "Installer already exists, but its immutable checksum sidecar does not match: $hashUrl"
        }

        Write-Host "Immutable release objects already have the expected bytes; skipping upload."
        $skipUpload = $true
    } elseif ($installerStatus -ne 404) {
        throw "Unexpected HTTP $installerStatus while checking immutable release object $installerUrl"
    }
} finally {
    Remove-Item -LiteralPath $existingInstallerPath, $existingHashPath -Force -ErrorAction SilentlyContinue
}

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

if (-not $skipUpload) {
    Publish-R2Object `
        -Path $installerPath `
        -ContentType "application/vnd.microsoft.portable-executable" `
        -CacheControl "public, max-age=31536000, immutable"
    Publish-R2Object `
        -Path $hashPath `
        -ContentType "text/plain; charset=utf-8" `
        -CacheControl "public, max-age=31536000, immutable"
}

if ($SkipPublicVerification) {
    Write-Output "Microsoft Store installer is available at: $installerUrl"
    return
}

$downloadPath = Join-Path ([System.IO.Path]::GetTempPath()) "ceiling-store-$Version-$([guid]::NewGuid().ToString('N')).exe"
try {
    # R2's public edge serves a newly written object a moment after the upload
    # API returns, so the first read can 404 on an object that is genuinely
    # there. curl's own --retry does not cover this: 404 is a permanent client
    # error, so it fails immediately. v1.5.17 died here 160 ms after a
    # successful upload, on a URL that served fine seconds later.
    #
    # Retry the whole request on any failure, backing off, and only give up
    # once the object has had a fair chance to propagate. A genuinely missing
    # object still fails, just later.
    # The loop owns the retry policy, so curl gets none of its own: nesting the
    # two multiplies the worst case (3 curl retries x 180s max-time, per outer
    # attempt) into something far longer than the window intended here.
    $maxAttempts = 8
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        & curl.exe `
            --fail `
            --silent `
            --show-error `
            --location `
            --max-redirs 0 `
            --connect-timeout 10 `
            --max-time 180 `
            --output $downloadPath `
            $installerUrl
        if ($LASTEXITCODE -eq 0) {
            break
        }
        if ($attempt -eq $maxAttempts) {
            throw "Direct public download failed with curl exit code $LASTEXITCODE after $maxAttempts attempts."
        }
        $backoff = [Math]::Min(5 * $attempt, 20)
        Write-Host "Public download attempt $attempt/$maxAttempts failed (curl exit $LASTEXITCODE); retrying in ${backoff}s."
        Start-Sleep -Seconds $backoff
    }

    $downloadHash = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($downloadHash -ne $expectedHash) {
        throw "Public installer SHA-256 mismatch. Expected $expectedHash, got $downloadHash."
    }
} finally {
    Remove-Item -LiteralPath $downloadPath -Force -ErrorAction SilentlyContinue
}

Write-Output "Verified direct Microsoft Store installer URL: $installerUrl"

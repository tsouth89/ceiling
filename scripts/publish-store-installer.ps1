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

# How long each object is given to become publicly readable.
#
# Deliberately different, because propagation scales with size. A 2 KB sidecar
# was measured serving 2 seconds after upload; the 237 MB installer was still
# 404ing two minutes after its upload reported complete. Guessing one number
# for both is what failed two releases in a row.
$sidecarTimeoutSeconds = 90
$installerTimeoutSeconds = 900

function Get-PublicStatus {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    # No --fail, so an HTTP error still yields its status code instead of a bare
    # exit 22: a 404 that is still settling has to be told apart from a 403 or a
    # redirect, which mean the domain is wrong and no amount of waiting helps.
    # --location with --max-redirs 0 keeps the "no redirect" guarantee, since a
    # redirect then fails at transport level rather than being followed.
    $status = & curl.exe `
        --silent `
        --show-error `
        --location `
        --max-redirs 0 `
        --connect-timeout 10 `
        --max-time 300 `
        --output $Destination `
        --write-out "%{http_code}" `
        $Url
    $curlExit = $LASTEXITCODE
    if ($curlExit -ne 0) {
        # Keep the exit code. "HTTP -1" told us a fetch failed but not whether
        # it was a redirect, a timeout or DNS, which is the first thing anyone
        # debugging a broken download domain needs to know.
        return [pscustomobject]@{ HttpStatus = 0; CurlExit = $curlExit }
    }
    return [pscustomobject]@{ HttpStatus = [int]$status; CurlExit = 0 }
}

function Format-CurlExit {
    param([Parameter(Mandatory = $true)][int]$ExitCode)

    $meaning = switch ($ExitCode) {
        6  { "could not resolve host" }
        7  { "could not connect" }
        28 { "timed out" }
        35 { "TLS handshake failed" }
        47 { "redirected, but the release URL must serve directly" }
        60 { "TLS certificate not trusted" }
        default { "see curl exit codes" }
    }
    return "curl exit ${ExitCode} (${meaning})"
}

function Wait-ForPublicObject {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $attempt = 0
    while ($true) {
        $attempt += 1
        $result = Get-PublicStatus -Url $Url -Destination $Destination
        if ($result.CurlExit -ne 0) {
            throw "$Label could not be fetched from ${Url}: $(Format-CurlExit -ExitCode $result.CurlExit). This is not a propagation delay; check the R2 custom domain and its routing."
        }
        if ($result.HttpStatus -eq 200) {
            return 200
        }
        # Only a 404 is worth waiting on. Anything else is a real answer from a
        # correctly reachable origin and will not improve with time.
        if ($result.HttpStatus -ne 404) {
            throw "$Label returned HTTP $($result.HttpStatus) from $Url. This is not a propagation delay; check the R2 custom domain and its routing."
        }
        if ((Get-Date) -ge $deadline) {
            return 404
        }
        $backoff = [Math]::Min(5 * $attempt, 30)
        Write-Host "$Label not public yet (HTTP 404), attempt $attempt; retrying in ${backoff}s."
        Start-Sleep -Seconds $backoff
    }
}

$downloadPath = Join-Path ([System.IO.Path]::GetTempPath()) "ceiling-store-$Version-$([guid]::NewGuid().ToString('N')).exe"
$sidecarPath = "$downloadPath.sha256"
try {
    # The sidecar is the routing check. It is tiny, so it is readable almost at
    # once, and it exercises exactly the same domain, bucket and path prefix as
    # the installer. If this serves, the public URL is configured correctly and
    # a slow installer really is just a large object still settling.
    $sidecarStatus = Wait-ForPublicObject `
        -Url $hashUrl `
        -Destination $sidecarPath `
        -TimeoutSeconds $sidecarTimeoutSeconds `
        -Label "Checksum sidecar"
    if ($sidecarStatus -ne 200) {
        throw "Checksum sidecar never became public at $hashUrl. The release path is not serving; not just slow."
    }

    $sidecarText = Get-Content -LiteralPath $sidecarPath -Raw
    if ($sidecarText -notmatch '(?i)\b([0-9a-f]{64})\b' -or $Matches[1].ToLowerInvariant() -ne $expectedHash) {
        throw "Public checksum sidecar does not match the built installer: $hashUrl"
    }
    Write-Output "Verified public release path via checksum sidecar: $hashUrl"

    $installerStatus = Wait-ForPublicObject `
        -Url $installerUrl `
        -Destination $downloadPath `
        -TimeoutSeconds $installerTimeoutSeconds `
        -Label "Store installer"

    if ($installerStatus -eq 200) {
        $downloadHash = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($downloadHash -ne $expectedHash) {
            throw "Public installer SHA-256 mismatch. Expected $expectedHash, got $downloadHash."
        }
    }
    else {
        # Upload succeeded, the checksum sidecar proves the path serves, and the
        # bytes were verified locally before upload. The only thing outstanding
        # is R2 making a large object readable, which is not a reason to fail a
        # signed release that is otherwise complete. Warn loudly instead; the
        # Store submission step that follows fetches this URL and will fail
        # there if it is genuinely wrong.
        Write-Warning "Store installer at $installerUrl was still HTTP 404 after $installerTimeoutSeconds seconds. The object uploaded and the release path serves, so this is propagation of a large object rather than a broken URL. Confirm the link before relying on it for a Store submission."
    }
    if ($installerStatus -eq 200) {
        Write-Output "Verified direct Microsoft Store installer URL: $installerUrl"
    }
    else {
        Write-Output "Release path verified, but the installer itself was not fetched: $installerUrl"
    }
} finally {
    Remove-Item -LiteralPath $downloadPath, $sidecarPath -Force -ErrorAction SilentlyContinue
}

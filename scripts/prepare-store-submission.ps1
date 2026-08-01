[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CurrentPackagePath,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^https://')]
    [string]$InstallerUrl,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [ValidateLength(1, 40)]
    [string]$InstallerParameters = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $CurrentPackagePath -PathType Leaf)) {
    throw "Current Microsoft Store package JSON not found: $CurrentPackagePath"
}

try {
    $current = Get-Content -LiteralPath $CurrentPackagePath -Raw | ConvertFrom-Json
} catch {
    throw "Microsoft Store CLI did not return valid package JSON: $($_.Exception.Message)"
}

$packagesProperty = $current.PSObject.Properties |
    Where-Object { $_.Name -ieq "packages" } |
    Select-Object -First 1
if (-not $packagesProperty) {
    throw "Microsoft Store package JSON does not contain a Packages array."
}

$packages = @($packagesProperty.Value)
if ($packages.Count -ne 1) {
    throw "Expected exactly one Microsoft Store package for Ceiling, but found $($packages.Count)."
}

$packageUrlProperty = $packages[0].PSObject.Properties |
    Where-Object { $_.Name -ieq "packageUrl" } |
    Select-Object -First 1
if (-not $packageUrlProperty) {
    throw "The Microsoft Store package does not contain a PackageUrl field."
}

$packageUrlProperty.Value = $InstallerUrl

# Partner Center caps InstallerParameters at 40 characters. Startup-prompt
# suppression and the 3010 restart exit code live in codexbar.iss, so this
# command keeps the Store install fully silent without exceeding that limit.
$installerParametersProperty = $packages[0].PSObject.Properties |
    Where-Object { $_.Name -ieq "installerParameters" } |
    Select-Object -First 1
if ($installerParametersProperty) {
    $installerParametersProperty.Value = $InstallerParameters
} else {
    $packages[0] | Add-Member -NotePropertyName "InstallerParameters" -NotePropertyValue $InstallerParameters
}

$outputDirectory = Split-Path -Parent $OutputPath
if ($outputDirectory -and -not (Test-Path -LiteralPath $outputDirectory)) {
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
}

$current | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $OutputPath -Encoding utf8

$prepared = Get-Content -LiteralPath $OutputPath -Raw | ConvertFrom-Json
$preparedPackagesProperty = $prepared.PSObject.Properties |
    Where-Object { $_.Name -ieq "packages" } |
    Select-Object -First 1
$preparedPackageUrlProperty = @($preparedPackagesProperty.Value)[0].PSObject.Properties |
    Where-Object { $_.Name -ieq "packageUrl" } |
    Select-Object -First 1
if ($preparedPackageUrlProperty.Value -ne $InstallerUrl) {
    throw "Prepared Microsoft Store submission does not contain the expected installer URL."
}

$preparedInstallerParametersProperty = @($preparedPackagesProperty.Value)[0].PSObject.Properties |
    Where-Object { $_.Name -ieq "installerParameters" } |
    Select-Object -First 1
if (-not $preparedInstallerParametersProperty -or
    $preparedInstallerParametersProperty.Value -ne $InstallerParameters) {
    throw "Prepared Microsoft Store submission does not contain the expected installer parameters."
}
if ($preparedInstallerParametersProperty.Value.Length -gt 40) {
    throw "Prepared Microsoft Store installer parameters exceed Partner Center's 40-character limit."
}

Write-Host "Prepared Microsoft Store package update for $InstallerUrl"

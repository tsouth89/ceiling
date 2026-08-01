#Requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("ceiling-store-test-" + [guid]::NewGuid())
$currentPath = Join-Path $tempRoot "current.json"
$preparedPath = Join-Path $tempRoot "prepared.json"
$missingParametersPath = Join-Path $tempRoot "missing-parameters.json"
$missingParametersPreparedPath = Join-Path $tempRoot "missing-parameters-prepared.json"
$installerUrl = "https://downloads.ceiling.win/releases/v1.5.21/Ceiling-1.5.21-Store-Setup.exe"
$expectedParameters = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"

try {
    New-Item -ItemType Directory -Path $tempRoot | Out-Null
    @{
        Packages = @(
            @{
                PackageUrl = "https://downloads.ceiling.win/releases/v1.5.19/Ceiling-1.5.19-Store-Setup.exe"
                Languages = @("en-us")
                Architectures = @("X64")
                IsSilentInstall = $false
                InstallerParameters = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /RESTARTEXITCODE=3010"
                PackageType = "exe"
            }
        )
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $currentPath -Encoding utf8

    & (Join-Path $repoRoot "scripts\prepare-store-submission.ps1") `
        -CurrentPackagePath $currentPath `
        -InstallerUrl $installerUrl `
        -OutputPath $preparedPath

    $prepared = Get-Content -LiteralPath $preparedPath -Raw | ConvertFrom-Json
    $package = @($prepared.Packages)[0]
    if ($package.PackageUrl -ne $installerUrl) {
        throw "Store preparation did not update PackageUrl."
    }
    if ($package.InstallerParameters -ne $expectedParameters) {
        throw "Store preparation did not normalize InstallerParameters."
    }
    if ($package.InstallerParameters.Length -ne 40) {
        throw "Expected 40-character Store installer parameters, got $($package.InstallerParameters.Length)."
    }
    if ($package.Architectures -ne "X64" -or $package.PackageType -ne "exe") {
        throw "Store preparation changed unrelated package metadata."
    }

    $package.PSObject.Properties.Remove("InstallerParameters")
    $prepared | ConvertTo-Json -Depth 10 |
        Set-Content -LiteralPath $missingParametersPath -Encoding utf8
    & (Join-Path $repoRoot "scripts\prepare-store-submission.ps1") `
        -CurrentPackagePath $missingParametersPath `
        -InstallerUrl $installerUrl `
        -OutputPath $missingParametersPreparedPath
    $missingParametersPackage = @(
        (Get-Content -LiteralPath $missingParametersPreparedPath -Raw | ConvertFrom-Json).Packages
    )[0]
    $propertyNames = @($missingParametersPackage.PSObject.Properties.Name)
    if ($propertyNames -cnotcontains "InstallerParameters" -or
        $propertyNames -ccontains "installerParameters") {
        throw "Store preparation added InstallerParameters with unexpected casing."
    }

    $installerScript = Get-Content -LiteralPath (Join-Path $repoRoot "rust\installer\codexbar.iss") -Raw
    if ($installerScript -notmatch '(?m)^DisableStartupPrompt=yes\r?$') {
        throw "Installer no longer suppresses the startup prompt internally."
    }
    if ($installerScript -notmatch '(?m)^function GetCustomSetupExitCode\(\): Integer;\r?$') {
        throw "Installer no longer returns 3010 for a successful install requiring restart."
    }

    Write-Host "Store submission parameters are valid and preserve installer behavior."
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}

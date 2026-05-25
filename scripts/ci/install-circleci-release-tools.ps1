#Requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$env:CARGO_TERM_COLOR = "never"
$env:CARGO_TERM_PROGRESS_WHEN = "never"
$env:RUSTUP_INIT_SKIP_PATH_CHECK = "yes"
$env:NO_COLOR = "1"
trap {
    Write-Host $_
    [Environment]::Exit(1)
}

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

function Test-Command {
    param([string]$Name)

    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-ChocoPackages {
    param([string[]]$Packages)

    if ($Packages.Count -eq 0) {
        return
    }

    choco feature enable -n allowGlobalConfirmation
    choco install @Packages -y --no-progress
}

function Add-CargoPath {
    if (Test-Path $cargoBin) {
        $env:Path = "$cargoBin;$env:Path"
    }
}

function Install-MinimalRustupToolchain {
    Write-Host "Ensuring minimal Rust MSVC toolchain..."

    if (-not (Test-Command "rustup")) {
        $rustupInit = Join-Path $env:TEMP "rustup-init.exe"
        $rustupUrl = "https://static.rust-lang.org/rustup/archive/1.27.1/x86_64-pc-windows-msvc/rustup-init.exe"
        $rustupChecksum = "193d6c727e18734edbf7303180657e96e9d5a08432002b4e6c5bbe77c60cb3e8"
        $chocoHelpers = Join-Path $env:ChocolateyInstall "helpers\chocolateyInstaller.psm1"

        if (-not (Test-Path $chocoHelpers)) {
            throw "Chocolatey helper module not found at $chocoHelpers"
        }

        Import-Module $chocoHelpers -Force
        Write-Host "Downloading rustup-init through Chocolatey helper..."
        Get-ChocolateyWebFile `
            -PackageName "rustup.install" `
            -FileFullPath $rustupInit `
            -Url64bit $rustupUrl `
            -Checksum64 $rustupChecksum `
            -ChecksumType64 "sha256" | Out-Null

        if (-not (Test-Path $rustupInit)) {
            throw "rustup-init download did not create $rustupInit"
        }

        Write-Host "Installing rustup without a default toolchain..."
        & $rustupInit -y --no-modify-path --profile minimal --default-toolchain none
        if ($LASTEXITCODE -ne 0) {
            throw "rustup-init failed with exit code $LASTEXITCODE"
        }

        Add-CargoPath
    }

    Write-Host "Installing/updating minimal stable MSVC toolchain..."
    rustup set profile minimal
    if ($LASTEXITCODE -ne 0) {
        throw "rustup set profile failed with exit code $LASTEXITCODE"
    }

    rustup toolchain install stable-x86_64-pc-windows-msvc --profile minimal --no-self-update
    if ($LASTEXITCODE -ne 0) {
        throw "rustup toolchain install failed with exit code $LASTEXITCODE"
    }

    rustup default stable-x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) {
        throw "rustup default failed with exit code $LASTEXITCODE"
    }
}

$fullRelease = $env:FULL_WINDOWS_RELEASE -eq "true"
$packages = @()
if (-not (Test-Command "git")) {
    $packages += "git"
}
if (-not (Test-Command "node")) {
    $packages += "nodejs-lts"
}
if ($fullRelease -and -not (Test-Command "gh")) {
    $packages += "gh"
}
if ($fullRelease -and -not (Test-Path (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"))) {
    $packages += "innosetup"
}

Install-ChocoPackages $packages

$env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
    [System.Environment]::GetEnvironmentVariable("Path", "User")
Add-CargoPath

Install-MinimalRustupToolchain

if (Test-Command "rustup") {
    rustup set auto-self-update disable
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Warning: rustup auto-self-update disable failed with exit code $LASTEXITCODE"
    }
} else {
    throw "Missing rustup after install/cache restore."
}

$env:CARGO_BUILD_TARGET = "x86_64-pc-windows-msvc"

corepack enable
if ($LASTEXITCODE -ne 0) {
    throw "corepack enable failed with exit code $LASTEXITCODE"
}

corepack prepare pnpm@10.18.1 --activate
if ($LASTEXITCODE -ne 0) {
    throw "corepack prepare failed with exit code $LASTEXITCODE"
}

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
    $vsInstall = & $vswhere -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
} else {
    $vsInstall = ""
}

if (-not $vsInstall) {
    throw "Missing Visual Studio C++ build tools. Select a CircleCI Windows image with MSVC installed or add a reviewed installer step."
}

git --version
cargo --version
rustc --version
pnpm --version

if ($fullRelease) {
    gh --version
    $iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
    if (-not (Test-Path $iscc)) {
        throw "Inno Setup compiler not found at $iscc"
    }
    Write-Host "Inno Setup compiler: $iscc"
} else {
    Write-Host "Skipping full-release tools for warm Windows build."
}

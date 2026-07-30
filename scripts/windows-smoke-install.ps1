param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [string]$ExpectedVersion = "",

    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\Ceiling",

    [switch]$LeaveInstalled,

    [switch]$RequireValidSignature,

    [string]$ExpectedSigner = "CN=Brandon South",

    # Must match CEILING_AUMID in rust/src/notifications.rs and AppUserModelId
    # in rust/installer/codexbar.iss.
    [string]$ExpectedAppUserModelId = "io.github.tsouth89.ceiling"
)

$ErrorActionPreference = "Stop"

function Write-Step {
    param([string]$Message)
    Write-Host "[smoke] $Message"
}

function Assert-Path {
    param(
        [string]$Path,
        [string]$Label
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Missing $Label at $Path"
    }
}

$isWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if (-not $isWindowsHost) {
    throw "This smoke test must run on Windows."
}

$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
if ([IO.Path]::GetExtension($installer).ToLowerInvariant() -ne ".exe") {
    throw "Expected an Inno Setup .exe installer, got: $installer"
}

Write-Step "installer: $installer"
$installerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
Write-Step "installer sha256: $installerHash"

$signature = Get-AuthenticodeSignature -FilePath $installer
if ($signature.Status -eq "Valid") {
    Write-Step "installer signature: valid ($($signature.SignerCertificate.Subject))"
} else {
    Write-Step "installer signature: $($signature.Status)"
}
if ($RequireValidSignature) {
    if ($signature.Status -ne "Valid") {
        throw "Installer Authenticode signature is not valid. Status: $($signature.Status)"
    }
    if ($signature.SignerCertificate.Subject -notlike "*$ExpectedSigner*") {
        throw "Unexpected installer signer: $($signature.SignerCertificate.Subject)"
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Installer is missing an RFC 3161 timestamp."
    }
}

foreach ($name in @("ceiling", "codexbar", "codexbar-desktop", "codexbar-desktop-tauri")) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | Stop-Process -Force
}

$logDir = Join-Path $env:TEMP "codexbar-installer-smoke"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$installLog = Join-Path $logDir "install.log"

Write-Step "running silent install"
$installArgs = @(
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/LOG=`"$installLog`""
)
$install = Start-Process -FilePath $installer -ArgumentList $installArgs -Wait -PassThru
if ($install.ExitCode -notin @(0, 3010)) {
    throw "Installer exited with $($install.ExitCode). Log: $installLog"
}

$desktopExe = Join-Path $InstallDir "ceiling.exe"
$cliExe = Join-Path $InstallDir "codexbar-cli.exe"
$icon = Join-Path $InstallDir "icon.ico"
Assert-Path -Path $desktopExe -Label "installed desktop executable"
Assert-Path -Path $cliExe -Label "installed CLI executable"
Assert-Path -Path $icon -Label "icon"

if ($RequireValidSignature) {
    foreach ($installedExe in @($desktopExe, $cliExe)) {
        $installedSignature = Get-AuthenticodeSignature -FilePath $installedExe
        if ($installedSignature.Status -ne "Valid") {
            throw "Installed executable signature is not valid: $installedExe ($($installedSignature.Status))"
        }
        if ($installedSignature.SignerCertificate.Subject -notlike "*$ExpectedSigner*") {
            throw "Unexpected installed executable signer: $($installedSignature.SignerCertificate.Subject)"
        }
        if ($null -eq $installedSignature.TimeStamperCertificate) {
            throw "Installed executable is missing an RFC 3161 timestamp: $installedExe"
        }
        Write-Step "installed signature: valid ($(Split-Path $installedExe -Leaf))"
    }
}

$desktopHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $desktopExe).Hash.ToLowerInvariant()
Write-Step "installed ceiling.exe sha256: $desktopHash"
$cliHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $cliExe).Hash.ToLowerInvariant()
Write-Step "installed codexbar-cli.exe sha256: $cliHash"

$verifyExecutablesScript = Join-Path (Split-Path -Parent $PSScriptRoot) "scripts\verify-windows-executables.ps1"
if (-not (Test-Path -LiteralPath $verifyExecutablesScript)) {
    throw "Executable verification script not found: $verifyExecutablesScript"
}
& $verifyExecutablesScript `
    -DesktopExe $desktopExe `
    -CliExe $cliExe `
    -CheckCliStdout

if ($ExpectedVersion) {
    $versionOutput = (& $cliExe --version) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw "codexbar-cli.exe --version exited with $LASTEXITCODE"
    }
    if ($versionOutput -notmatch [regex]::Escape($ExpectedVersion)) {
        throw "Expected codexbar-cli.exe --version to mention $ExpectedVersion, got: $versionOutput"
    }
    Write-Step "CLI version output: $versionOutput"
}

$helpOutput = (& $cliExe --help) -join "`n"
if ($LASTEXITCODE -ne 0) {
    throw "codexbar-cli.exe --help exited with $LASTEXITCODE"
}
if ($helpOutput -notmatch "Usage:" -or $helpOutput -notmatch "diagnose") {
    throw "codexbar-cli.exe --help did not print CLI help."
}
Write-Step "CLI help output: ok"

$uninstallKeys = @(
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\io.github.tsouth89.ceiling_is1",
    "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\io.github.tsouth89.ceiling_is1",
    "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\io.github.tsouth89.ceiling_is1"
)
$uninstallEntry = $null
foreach ($key in $uninstallKeys) {
    if (Test-Path $key) {
        $uninstallEntry = Get-ItemProperty $key
        break
    }
}
if ($null -eq $uninstallEntry) {
    throw "Missing Ceiling uninstall registry entry."
}

Write-Step "registry display name: $($uninstallEntry.DisplayName)"
if ($ExpectedVersion -and $uninstallEntry.DisplayVersion -ne $ExpectedVersion) {
    throw "Expected DisplayVersion $ExpectedVersion, got $($uninstallEntry.DisplayVersion)"
}

$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$shortcutCandidates = @(
    (Join-Path $startMenu "Ceiling.lnk"),
    (Join-Path $startMenu "Ceiling\Ceiling.lnk")
)
$shortcut = $shortcutCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $shortcut) {
    throw "Missing Start Menu shortcut. Checked: $($shortcutCandidates -join ', ')"
}
Write-Step "Start Menu shortcut: $shortcut"

# The shortcut existing is not the same as it carrying an identity. Windows
# keeps a toast in the notification center only for an app it can resolve, and
# it resolves that from System.AppUserModel.ID on this shortcut. 1.5.17 shipped
# with the installer setting no such property: every alert drew a banner and was
# then discarded. Nothing caught it, because every other gate checks source
# while this is the only step that inspects what was actually installed.
# -TypeDefinition, not -MemberDefinition: this declares types, which the
# member form cannot host.
if (-not ('Ceiling.Lnk' -as [type])) {
    Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace Ceiling {
  [ComImport, Guid("00021401-0000-0000-C000-000000000046")] internal class ShellLink {}
  [ComImport, Guid("0000010b-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  internal interface IPersistFile {
    void GetClassID(out Guid pClassID); [PreserveSig] int IsDirty();
    void Load([MarshalAs(UnmanagedType.LPWStr)] string fileName, int mode);
    void Save([MarshalAs(UnmanagedType.LPWStr)] string fileName, [MarshalAs(UnmanagedType.Bool)] bool remember);
    void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string fileName);
    void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string fileName); }
  [StructLayout(LayoutKind.Sequential)] internal struct PropertyKey { public Guid fmtid; public uint pid; }
  [ComImport, Guid("886d8eeb-8cf2-4446-8d02-cdba1dbdcf99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  internal interface IPropertyStore {
    void GetCount(out uint count); void GetAt(uint index, out PropertyKey key);
    void GetValue(ref PropertyKey key, [In, Out] PropVariant value);
    void SetValue(ref PropertyKey key, PropVariant value); void Commit(); }
  [StructLayout(LayoutKind.Explicit)] internal class PropVariant : IDisposable {
    private const ushort VT_EMPTY = 0;
    private const ushort VT_LPWSTR = 31;
    [FieldOffset(0)] ushort valueType; [FieldOffset(8)] IntPtr pointer;
    public string AsString() {
      return valueType == VT_LPWSTR && pointer != IntPtr.Zero ? Marshal.PtrToStringUni(pointer) : ""; }
    // Free only what is actually a CoTaskMem string. The field at offset 8
    // overlaps a union, so a non-string variant can leave arbitrary non-zero
    // bits there, and handing those to FreeCoTaskMem corrupts the heap.
    // Anything else is left alone: leaking a few bytes in a short-lived check
    // is strictly better than freeing memory that was never allocated.
    public void Dispose() {
      if (valueType == VT_LPWSTR && pointer != IntPtr.Zero) { Marshal.FreeCoTaskMem(pointer); }
      valueType = VT_EMPTY; pointer = IntPtr.Zero; } }
  public static class Lnk {
    public static string ReadAppUserModelId(string path) {
      var key = new PropertyKey { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
      var link = new ShellLink();
      ((IPersistFile)link).Load(path, 0);
      using (var value = new PropVariant()) {
        ((IPropertyStore)link).GetValue(ref key, value);
        return value.AsString(); } } } }
'@
}
$shortcutAumid = [Ceiling.Lnk]::ReadAppUserModelId($shortcut)
if ($shortcutAumid -ne $ExpectedAppUserModelId) {
    throw "Start Menu shortcut publishes AppUserModelID '$shortcutAumid' but Ceiling toasts under '$ExpectedAppUserModelId'. Windows will show alerts as banners and then discard them instead of keeping them in the notification center. Check AppUserModelID on the [Icons] entries in rust/installer/codexbar.iss."
}
Write-Step "shortcut AppUserModelID: $shortcutAumid"

if (-not $LeaveInstalled) {
    $uninstallLog = Join-Path $logDir "uninstall.log"
    $uninstallCommand = [string]$uninstallEntry.UninstallString
    if (-not $uninstallCommand) {
        throw "UninstallString is empty."
    }

    $uninstaller = $uninstallCommand.Trim('"')
    Write-Step "running silent uninstall"
    $uninstallArgs = @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/LOG=`"$uninstallLog`""
    )
    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList $uninstallArgs -Wait -PassThru
    if ($uninstall.ExitCode -notin @(0, 3010)) {
        throw "Uninstaller exited with $($uninstall.ExitCode). Log: $uninstallLog"
    }
    foreach ($leftover in @($desktopExe, $cliExe)) {
        if (Test-Path -LiteralPath $leftover) {
            throw "Executable still exists after uninstall: $leftover"
        }
    }
}

Write-Step "ok"

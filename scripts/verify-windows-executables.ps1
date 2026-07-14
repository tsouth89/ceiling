param(
    [Parameter(Mandatory = $true)]
    [string]$DesktopExe,

    [Parameter(Mandatory = $true)]
    [string]$CliExe,

    [string]$LegacyDesktopExe = "",

    [switch]$CheckCliStdout
)

$ErrorActionPreference = "Stop"

function Resolve-RequiredPath {
    param(
        [string]$Path,
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Missing $Label at $Path"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Get-PeSubsystem {
    param([string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        try {
            [void]$stream.Seek(0x3c, [System.IO.SeekOrigin]::Begin)
            $peOffset = $reader.ReadInt32()
            [void]$stream.Seek($peOffset + 4 + 20 + 68, [System.IO.SeekOrigin]::Begin)
            return $reader.ReadUInt16()
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

$desktop = Resolve-RequiredPath -Path $DesktopExe -Label "desktop executable"
$cli = Resolve-RequiredPath -Path $CliExe -Label "CLI executable"
$legacyDesktop = if ($LegacyDesktopExe) {
    Resolve-RequiredPath -Path $LegacyDesktopExe -Label "desktop compatibility executable"
} else {
    $null
}

$desktopHash = Get-Sha256 $desktop
$cliHash = Get-Sha256 $cli
$legacyDesktopHash = if ($legacyDesktop) { Get-Sha256 $legacyDesktop } else { "" }

if ($desktopHash -eq $cliHash) {
    throw "The Ceiling desktop executable and codexbar-cli.exe must not be byte-identical; the CLI must be the console binary."
}
if ($legacyDesktop -and $desktopHash -ne $legacyDesktopHash) {
    throw "The Ceiling desktop executable and compatibility desktop executable should be identical."
}

$desktopSubsystem = Get-PeSubsystem $desktop
$cliSubsystem = Get-PeSubsystem $cli
$legacyDesktopSubsystem = if ($legacyDesktop) { Get-PeSubsystem $legacyDesktop } else { $null }

if ($desktopSubsystem -ne 2) {
    throw "The Ceiling desktop executable must be a Windows GUI-subsystem binary; got subsystem $desktopSubsystem."
}
if ($legacyDesktop -and $legacyDesktopSubsystem -ne 2) {
    throw "The compatibility desktop executable must use the Windows GUI subsystem; got subsystem $legacyDesktopSubsystem."
}
if ($cliSubsystem -ne 3) {
    throw "codexbar-cli.exe must be a Windows console-subsystem CLI binary; got subsystem $cliSubsystem."
}

if ($CheckCliStdout) {
    $stdoutPath = Join-Path ([System.IO.Path]::GetTempPath()) "codexbar-cli-stdout-$PID.txt"
    try {
        & $cli --help > $stdoutPath
        if ($LASTEXITCODE -ne 0) {
            throw "codexbar-cli.exe --help exited with $LASTEXITCODE"
        }
        $stdout = Get-Content -Raw -LiteralPath $stdoutPath
        if (-not $stdout -or $stdout -notmatch "Usage:" -or $stdout -notmatch "diagnose") {
            throw "codexbar-cli.exe --help did not write expected CLI help to redirected stdout."
        }
    } finally {
        Remove-Item -LiteralPath $stdoutPath -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Windows executable layout verified: desktop GUI binary + separate console CLI."

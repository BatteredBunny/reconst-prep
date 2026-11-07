<#
.SYNOPSIS
Installs what building reconst-prep and its Velopack installer needs on Windows.

.DESCRIPTION
CI runs this too. Safe to re-run: tools that are already present are skipped.

Without -InstallPrerequisites it only checks Rust (MSVC), the MSVC build tools,
Python 3, and the .NET SDK, and stops with the winget commands for whatever is
missing. With it, the missing ones are installed through winget.

Then installs the Rust toolchain pinned in rust-toolchain.toml and the
Velopack CLI (vpk) pinned in .config/dotnet-tools.json.

.PARAMETER InstallPrerequisites
Install missing prerequisites (and ffmpeg) with winget.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\install-deps-windows.ps1 -InstallPrerequisites
#>
[CmdletBinding()]
param(
    [switch]$InstallPrerequisites
)

$ErrorActionPreference = "Stop"

# Windows PowerShell 5.1 does not stop on a failing native command, even with ErrorActionPreference.
function Invoke-Native {
    param([string]$Exe, [string[]]$Arguments)
    Write-Host "+ $Exe $($Arguments -join ' ')"
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Exe exited with $LASTEXITCODE"
    }
}

function Test-Command([string]$Name) {
    [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Test-Msvc {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        return $false
    }
    $path = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    [bool]$path
}

$Prerequisites = @(
    @{
        Name = "Rust (rustup)"
        Present = { Test-Command rustup }
        Winget = @("Rustlang.Rustup")
    },
    @{
        Name = "MSVC build tools"
        Present = { Test-Msvc }
        Winget = @("Microsoft.VisualStudio.2022.BuildTools", "--override",
            "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
    },
    @{
        Name = "Python 3 (for Stylo code generation)"
        Present = { (Test-Command python) -and ((& python -c "import sys; print(sys.version_info.major)" 2>$null) -eq "3") }
        Winget = @("Python.Python.3.13")
    },
    @{
        Name = ".NET SDK (for vpk)"
        Present = { Test-Command dotnet }
        Winget = @("Microsoft.DotNet.SDK.10")
    }
)

$Missing = @($Prerequisites | Where-Object { -not (& $_.Present) })
foreach ($p in $Prerequisites) {
    if ($Missing -contains $p) { $state = "missing" } else { $state = "ok" }
    Write-Host ("{0,-8} {1}" -f $state, $p.Name)
}

$WantFfmpeg = $InstallPrerequisites -and -not (Test-Command ffmpeg)

if ($Missing.Count -gt 0 -or $WantFfmpeg) {
    if (-not $InstallPrerequisites) {
        Write-Host ""
        Write-Host "Install the missing prerequisites, or re-run with -InstallPrerequisites:"
        foreach ($p in $Missing) {
            $shown = $p.Winget | ForEach-Object { if ($_ -match ' ') { "`"$_`"" } else { $_ } }
            Write-Host "  winget install --exact --id $($shown -join ' ')"
        }
        throw "missing prerequisites"
    }
    if (-not (Test-Command winget)) {
        throw "winget not found; install App Installer from the Microsoft Store, or install the prerequisites by hand"
    }
    foreach ($p in $Missing) {
        Invoke-Native winget (@("install", "--exact", "--accept-source-agreements", "--accept-package-agreements", "--id") + $p.Winget)
    }
    if ($WantFfmpeg) {
        Invoke-Native winget @("install", "--exact", "--accept-source-agreements", "--accept-package-agreements", "--id", "Gyan.FFmpeg")
    }
    if ($Missing.Count -gt 0) {
        # winget does not refresh this session's PATH, so rustup and dotnet are not visible yet.
        Write-Host ""
        Write-Host "Prerequisites installed. Open a new terminal and run this script again to install Rust and vpk."
        exit 0
    }
}

Push-Location (Split-Path $PSScriptRoot)
try {
    # With no argument, rustup installs the toolchain pinned in rust-toolchain.toml.
    Invoke-Native rustup @("toolchain", "install")
    Invoke-Native dotnet @("tool", "restore")
    $RustHost = (rustc -vV | Select-String '^host: (.*)').Matches[0].Groups[1].Value
} finally {
    Pop-Location
}
if ($RustHost -notmatch "msvc") {
    Write-Warning "Rust host is '$RustHost'; the build expects x86_64-pc-windows-msvc (rustup set default-host x86_64-pc-windows-msvc)"
}

if (-not (Test-Command ffmpeg)) {
    Write-Host "note: ffmpeg is not on PATH. reconst-prep needs it at runtime (the installer bundles it);"
    Write-Host "      re-run with -InstallPrerequisites, or pass --ffmpeg-path to reconst-prep."
}

Write-Host "Dependencies installed."

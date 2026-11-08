# Copies the pinned ffmpeg build (executables, DLLs, licence) into a package directory.
# Shared by the installer and the portable command line zip, so both ship the same ffmpeg.
param(
    [Parameter(Mandatory)] [string] $Destination
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$FfmpegVersion = "9.0.2"
$FfmpegZip = "ffmpeg-$FfmpegVersion-full_build-shared.zip"
$FfmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$FfmpegVersion/$FfmpegZip"
# The digest GitHub publishes for this release asset.
$FfmpegSha256 = "8d31e162f1616e37aab3fa2db991b97e1b8dbeb1c8465fd81a81be16ff91b328"

if (-not (Test-Path $FfmpegZip)) {
    Invoke-WebRequest -Uri $FfmpegUrl -OutFile $FfmpegZip
}
$Actual = (Get-FileHash -Algorithm SHA256 $FfmpegZip).Hash
if ($Actual -ne $FfmpegSha256) {
    Remove-Item $FfmpegZip
    throw "$FfmpegZip has SHA-256 $Actual, expected $FfmpegSha256 (deleted; re-run to download again)"
}
Expand-Archive $FfmpegZip -DestinationPath ffmpeg-tmp -Force
# reconst-prep looks for ffmpeg beside its own executable first.
Copy-Item ffmpeg-tmp\*\bin\* $Destination -Exclude ffplay.exe
$FfmpegLicense = Get-ChildItem ffmpeg-tmp\*\LICENSE* | Select-Object -First 1
if (-not $FfmpegLicense) {
    throw "failed to find the license"
}
Copy-Item $FfmpegLicense (Join-Path $Destination "LICENSE-ffmpeg.txt")
Remove-Item -Recurse -Force ffmpeg-tmp

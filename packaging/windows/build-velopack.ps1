$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Version = (Select-String -Path ..\..\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

$FfmpegRelease = "autobuild-2026-07-31-12-50"
$FfmpegZip = "ffmpeg-n8.0-latest-win64-lgpl-shared-8.0.zip"
$FfmpegUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$FfmpegRelease/$FfmpegZip"

Push-Location ..\..
cargo build --locked --release -p reconst-prep
Pop-Location

$Publish = "publish"
Remove-Item -Recurse -Force $Publish -ErrorAction SilentlyContinue
New-Item -ItemType Directory $Publish | Out-Null
Copy-Item ..\..\target\release\reconst-prep.exe $Publish
Copy-Item ..\..\LICENSE $Publish

if (-not (Test-Path $FfmpegZip)) {
    Invoke-WebRequest -Uri $FfmpegUrl -OutFile $FfmpegZip
}
Expand-Archive $FfmpegZip -DestinationPath ffmpeg-tmp -Force
# bin/ contains ffmpeg.exe, ffprobe.exe and the LGPL DLLs they need.
Copy-Item ffmpeg-tmp\*\bin\* $Publish
Copy-Item ffmpeg-tmp\*\LICENSE.txt $Publish\LICENSE-ffmpeg.txt -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force ffmpeg-tmp

# Velopack CLI: dotnet tool install -g vpk
vpk pack `
    --packId ReconstPrep `
    --packVersion $Version `
    --packDir $Publish `
    --mainExe reconst-prep.exe `
    --packTitle "reconst-prep" `
    --icon reconst-prep.ico `
    --outputDir releases

Write-Host "installer in packaging/windows/releases/"

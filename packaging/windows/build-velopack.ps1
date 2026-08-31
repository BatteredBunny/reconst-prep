$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Version = (Select-String -Path ..\..\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

$FfmpegVersion = "9.0.1"
$FfmpegZip = "ffmpeg-$FfmpegVersion-full_build-shared.zip"
$FfmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$FfmpegVersion/$FfmpegZip"

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
Copy-Item ffmpeg-tmp\*\bin\* $Publish -Exclude ffplay.exe
$FfmpegLicense = Get-ChildItem ffmpeg-tmp\*\LICENSE* | Select-Object -First 1
if (-not $FfmpegLicense) {
    throw "failed to find the license"
}
Copy-Item $FfmpegLicense $Publish\LICENSE-ffmpeg.txt
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

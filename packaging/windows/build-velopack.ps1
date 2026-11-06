$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Version = (Select-String -Path ..\..\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

$FfmpegVersion = "9.0.1"
$FfmpegZip = "ffmpeg-$FfmpegVersion-full_build-shared.zip"
$FfmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$FfmpegVersion/$FfmpegZip"
$FfmpegSha256 = "6fd54b3b4f49117a307877b570f5e1659090f178973298658b41f5c559b5b5ab"

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
$Actual = (Get-FileHash -Algorithm SHA256 $FfmpegZip).Hash
if ($Actual -ne $FfmpegSha256) {
    Remove-Item $FfmpegZip
    throw "$FfmpegZip has SHA-256 $Actual, expected $FfmpegSha256 (deleted; re-run to download again)"
}
Expand-Archive $FfmpegZip -DestinationPath ffmpeg-tmp -Force
Copy-Item ffmpeg-tmp\*\bin\* $Publish -Exclude ffplay.exe
$FfmpegLicense = Get-ChildItem ffmpeg-tmp\*\LICENSE* | Select-Object -First 1
if (-not $FfmpegLicense) {
    throw "failed to find the license"
}
Copy-Item $FfmpegLicense $Publish\LICENSE-ffmpeg.txt
Remove-Item -Recurse -Force ffmpeg-tmp

# Pinned in .config/dotnet-tools.json; scripts/install-deps-windows.ps1 restores it.
dotnet vpk pack `
    --packId ReconstPrep `
    --packVersion $Version `
    --packDir $Publish `
    --mainExe reconst-prep.exe `
    --packTitle "reconst-prep" `
    --icon reconst-prep.ico `
    --outputDir releases

Write-Host "installer in packaging/windows/releases/"

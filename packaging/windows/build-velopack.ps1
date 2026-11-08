$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Version = (Select-String -Path ..\..\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

Push-Location ..\..
cargo build --locked --release -p reconst-prep
Pop-Location

$Publish = "publish"
Remove-Item -Recurse -Force $Publish -ErrorAction SilentlyContinue
New-Item -ItemType Directory $Publish | Out-Null
Copy-Item ..\..\target\release\reconst-prep.exe $Publish
Copy-Item ..\..\LICENSE $Publish

& .\fetch-ffmpeg.ps1 -Destination $Publish

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

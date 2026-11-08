# The command line alone, as a zip to unpack anywhere: no installer, no `gui` subcommand.
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Version = (Select-String -Path ..\..\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value
$Name = "reconst-prep-cli-$Version-win-x64"

Push-Location ..\..
cargo build --locked --release -p reconst-prep --no-default-features
Pop-Location

$Publish = "publish-cli\$Name"
Remove-Item -Recurse -Force publish-cli -ErrorAction SilentlyContinue
New-Item -ItemType Directory $Publish | Out-Null
Copy-Item ..\..\target\release\reconst-prep.exe $Publish
Copy-Item ..\..\LICENSE $Publish

& .\fetch-ffmpeg.ps1 -Destination $Publish

New-Item -ItemType Directory releases -Force | Out-Null
# One top-level folder, so unpacking never scatters DLLs into the current directory.
Compress-Archive -Path $Publish -DestinationPath "releases\$Name.zip" -Force

Write-Host "zip in packaging/windows/releases/$Name.zip"

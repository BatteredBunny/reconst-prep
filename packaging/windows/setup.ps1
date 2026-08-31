$ErrorActionPreference = "Stop"

if ((dotnet tool list -g) -match '^vpk\s') {
    dotnet tool update -g vpk
} else {
    dotnet tool install -g vpk
}

$ToolPath = Join-Path $env:USERPROFILE ".dotnet\tools"
if ($env:GITHUB_PATH) {
    $ToolPath | Out-File -Append -Encoding utf8 $env:GITHUB_PATH
} elseif (-not (Get-Command vpk -ErrorAction SilentlyContinue)) {
    Write-Host "vpk is installed but not on PATH; add $ToolPath"
}

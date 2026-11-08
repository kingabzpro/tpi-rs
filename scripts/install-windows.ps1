# Download and install a TPI-RS release on Windows without Rust.
# Review this script before running it.

param(
    [string]$Version = "latest",
    [string]$InstallRoot = "$env:LOCALAPPDATA\TPI-RS"
)

$ErrorActionPreference = "Stop"
$repo = "kingabzpro/tpi-rs"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if ($Version -eq "latest") {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers @{ "User-Agent" = "tpi-installer" }
} else {
    $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/tags/$tag" -Headers @{ "User-Agent" = "tpi-installer" }
}

$tagName = $release.tag_name
$asset = $release.assets | Where-Object { $_.name -like "*x86_64-pc-windows-msvc.zip" } | Select-Object -First 1
$checksumAsset = $release.assets | Where-Object { $_.name -eq "$($asset.name).sha256" } | Select-Object -First 1

if (-not $asset) {
    throw "No Windows x86_64 asset found on release $tagName."
}

$installDir = Join-Path $InstallRoot $tagName
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$zipPath = Join-Path $env:TEMP $asset.name

Write-Host "Downloading $($asset.name)"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -UseBasicParsing

if ($checksumAsset) {
    $sumPath = "$zipPath.sha256"
    Invoke-WebRequest -Uri $checksumAsset.browser_download_url -OutFile $sumPath -UseBasicParsing
    $expected = (Get-Content $sumPath -Raw).Split(" ")[0].Trim().ToLower()
    $actual = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        Remove-Item $zipPath -Force
        throw "Checksum mismatch. Download discarded."
    }
    Write-Host "Checksum verified."
} else {
    Write-Warning "This release has no checksum asset. Download not verified."
}

Expand-Archive -Path $zipPath -DestinationPath $installDir -Force
Remove-Item $zipPath -Force
Remove-Item "$zipPath.sha256" -Force -ErrorAction SilentlyContinue

$exe = Join-Path $installDir "tpi.exe"
if (-not (Test-Path $exe)) {
    throw "Install finished but tpi.exe was not found in $installDir."
}

Write-Host ""
Write-Host "Installed TPI-RS $tagName"
Write-Host "  Binary: $exe"
Write-Host "  Config: $(Join-Path $installDir 'config\default.toml')"
Write-Host ""
Write-Host "Next:"
Write-Host "  cd `"$installDir`""
Write-Host "  Copy-Item .env.example .env"
Write-Host "  .\tpi.exe --doctor"
Write-Host "  .\tpi.exe --offline --tickers AAPL,MSFT --mode scan"
Write-Host ""
Write-Host "Add this folder to PATH if you want to run tpi from anywhere:"
Write-Host "  $installDir"

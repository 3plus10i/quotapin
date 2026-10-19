# Builds QuotaPin and stages a portable zip under dist/.
#
#   pwsh -NoProfile -File scripts/build.ps1
#   pwsh -NoProfile -File scripts/build.ps1 -SkipTests
param([switch]$SkipTests)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host '== cargo build --release'
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }

    if (-not $SkipTests) {
        Write-Host '== cargo test --release'
        cargo test --release
        if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
    }

    $version = (Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
    $dist = Join-Path $root 'dist'
    $stage = Join-Path $dist 'QuotaPin'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item 'target/release/QuotaPin.exe' $stage
    Copy-Item 'README.md' $stage
    Copy-Item 'assets/logo.png' $stage

    $zip = Join-Path $dist "QuotaPin-$version-win64.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

    $exe = Get-Item 'target/release/QuotaPin.exe'
    $kb = { param($f) [math]::Round((Get-Item $f).Length / 1KB, 1) }
    Write-Host ''
    Write-Host ("exe : {0} KB" -f (& $kb $exe.FullName))
    Write-Host ("zip : {0} KB -> {1}" -f (& $kb $zip), $zip)
}
finally {
    Pop-Location
}

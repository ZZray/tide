param([string]$Executable, [string]$OutputDirectory, [string]$Iscc)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    $workspace = Get-Content Cargo.toml -Raw
    $version = [regex]::Match($workspace,'(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"').Groups[1].Value
    if (!$version) { throw 'Cannot determine package version.' }
    if (!$Executable) {
        $metadataText = & cargo metadata --locked --format-version 1 --no-deps
        if ($LASTEXITCODE) { throw 'Cannot determine Cargo target directory.' }
        $Executable = Join-Path (($metadataText | ConvertFrom-Json).target_directory) 'release/tide.exe'
    }
    $Executable = (Resolve-Path -LiteralPath $Executable).Path
    if (!$OutputDirectory) { $OutputDirectory = Join-Path $repo 'target/dist' }
    $OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
    $stage = Join-Path $OutputDirectory "Tide-v$version-windows-x64"
    if (Test-Path -LiteralPath $stage) { throw 'Package staging directory already exists; choose a fresh output directory.' }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath $Executable -Destination (Join-Path $stage 'tide.exe')
    foreach ($file in @('README.md','README.zh-CN.md','LICENSE','THIRD_PARTY_NOTICES.md','CONTRIBUTING.md')) { Copy-Item -LiteralPath (Join-Path $repo $file) -Destination $stage }
    New-Item -ItemType Directory -Path (Join-Path $stage 'examples'),(Join-Path $stage 'assets/brand'),(Join-Path $stage 'docs') -Force | Out-Null
    Copy-Item -LiteralPath examples/config.toml -Destination (Join-Path $stage 'examples')
    Copy-Item -LiteralPath docs/preferences-sync.md -Destination (Join-Path $stage 'docs')
    Copy-Item -LiteralPath docs/development.md -Destination (Join-Path $stage 'docs')
    Copy-Item -LiteralPath licenses -Destination $stage -Recurse
    Copy-Item -Path assets/brand/* -Destination (Join-Path $stage 'assets/brand')
    & "$PSScriptRoot/collect-licenses.ps1" -Destination (Join-Path $stage 'THIRD-PARTY-LICENSES.txt')
    if (!$Iscc) { $Iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe' }
    if (!(Test-Path -LiteralPath $Iscc)) { throw 'Install Inno Setup 6 or pass -Iscc.' }
    & $Iscc "/DAppVersion=$version" "/DSourceDir=$stage" "/DOutputDir=$OutputDirectory" packaging/windows/tide.iss
    if ($LASTEXITCODE) { throw 'Installer compilation failed.' }
    $portable = Join-Path $OutputDirectory "Tide-v$version-windows-x64-portable.zip"
    Compress-Archive -LiteralPath $stage -DestinationPath $portable -CompressionLevel Optimal
    $installer = Join-Path $OutputDirectory "Tide-v$version-windows-x64-setup.exe"
    $hashes = foreach ($file in @($installer,$portable)) { "{0}  {1}" -f (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant(),[IO.Path]::GetFileName($file) }
    [IO.File]::WriteAllLines((Join-Path $OutputDirectory 'SHA256SUMS.txt'),$hashes,[Text.UTF8Encoding]::new($false))
    Get-Item -LiteralPath $installer,$portable,(Join-Path $OutputDirectory 'SHA256SUMS.txt') | Select-Object Name,Length
} finally { Pop-Location }

param([Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference = 'Stop'
$metadataText = & cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE) { throw 'Could not read locked Cargo metadata.' }
$metadata = $metadataText | ConvertFrom-Json
$fallbackDirectory = Join-Path (Split-Path $PSScriptRoot -Parent) 'licenses/upstream'
$fallbacks = Get-Content -LiteralPath (Join-Path $fallbackDirectory 'index.json') -Raw | ConvertFrom-Json
$text = [Text.StringBuilder]::new()
[void]$text.AppendLine('Tide third-party license inventory / 第三方许可清单')
[void]$text.AppendLine('Generated from Cargo.lock and original dependency sources. Alternative license texts may be included.')
foreach ($package in ($metadata.packages | Where-Object source | Sort-Object name,version)) {
    $root = Split-Path $package.manifest_path -Parent
    [void]$text.AppendLine("`n===== $($package.name) $($package.version) =====")
    [void]$text.AppendLine("SPDX: $($package.license)`nRepository: $($package.repository)`nAuthors: $($package.authors -join ', ')")
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File | Where-Object {
        $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([._-]|$)' -or
        $_.FullName -match '[\\/]LICENSES[\\/]'
    } | Sort-Object FullName)
    if ($package.license_file) {
        $explicit = if ([IO.Path]::IsPathRooted($package.license_file)) { $package.license_file } else { Join-Path $root $package.license_file }
        if (Test-Path -LiteralPath $explicit -PathType Leaf) { $files += Get-Item -LiteralPath $explicit }
    }
    if (!$files.Count) {
        $fallback = @($fallbacks | Where-Object { $_.name -eq $package.name -and $_.version -eq $package.version })
        if ($fallback.Count -ne 1 -or $fallback[0].license -ne $package.license) { throw "Missing reviewed upstream license for $($package.name) $($package.version)" }
        $entry = $fallback[0]
        if ($entry.file -notmatch '^[a-zA-Z0-9_.-]+\.txt$' -or !$entry.sources.Count -or $entry.source_commit -notmatch '^[a-f0-9]{40}$') { throw 'Invalid license fallback manifest entry.' }
        $licensePath = Join-Path $fallbackDirectory $entry.file
        if ((Get-FileHash -LiteralPath $licensePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256) { throw "License fallback hash mismatch: $($entry.file)" }
        [void]$text.AppendLine([IO.File]::ReadAllText($licensePath))
    }
    foreach ($file in ($files | Sort-Object FullName -Unique)) {
        [void]$text.AppendLine("`n--- $([IO.Path]::GetRelativePath($root,$file.FullName)) ---")
        [void]$text.AppendLine([IO.File]::ReadAllText($file.FullName))
    }
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($Destination),$text.ToString(),[Text.UTF8Encoding]::new($false))
# Rust distributions provide their standard-library copyright/license bundle.
$sysroot = & rustc --print sysroot
if ($LASTEXITCODE) { throw 'Could not locate Rust license bundle.' }
$rustNotices = Join-Path $sysroot 'share/doc/rust'
$rustDestination = Join-Path (Split-Path $Destination -Parent) 'licenses/rust'
New-Item -ItemType Directory -Path $rustDestination -Force | Out-Null
foreach ($name in @('COPYRIGHT-library.html','COPYRIGHT.html','licenses')) {
    $source = Join-Path $rustNotices $name
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $rustDestination -Recurse -Force }
}
if (!(Test-Path -LiteralPath (Join-Path $rustDestination 'COPYRIGHT-library.html'))) { throw 'Rust standard-library notices are missing.' }

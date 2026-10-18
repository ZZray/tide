$CargoArgs = if ($args.Count) { @($args) } else { @('test', '--workspace') }
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$sharedRust = 'D:\workspace\env\tools\rust'
$rustCache = 'F:\cache\rust'
if (-not (Test-Path -LiteralPath 'F:\')) { throw 'Rust cache drive F: is unavailable; refusing to fall back to C:.' }
New-Item -ItemType Directory -Path "$rustCache\cargo", "$rustCache\target", "$rustCache\build", "$rustCache\tmp" -Force | Out-Null
$env:CARGO_HOME = "$rustCache\cargo"
$env:CARGO_TARGET_DIR = "$rustCache\target"
$env:CARGO_BUILD_BUILD_DIR = "$rustCache\build\{workspace-path-hash}"
# Keep Rust subprocess scratch files off the system drive without changing
# the user's TEMP/TMP for other applications.
$env:TEMP = "$rustCache\tmp"
$env:TMP = "$rustCache\tmp"
if (Test-Path "$sharedRust\cargo\bin\cargo.exe") {
    $env:RUSTUP_HOME = "$sharedRust\rustup"
    $env:Path = "$sharedRust\cargo\bin;$env:Path"
}
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsRoot = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vsRoot) {
            $vsCommand = '"' + $vsRoot + '\Common7\Tools\VsDevCmd.bat" -no_logo -arch=x64 -host_arch=x64 >nul && set'
            & cmd.exe /d /s /c $vsCommand | ForEach-Object {
                if ($_ -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
            }
        }
    }
}
Push-Location $repoRoot
try {
    & cargo @CargoArgs
    $cargoExit = $LASTEXITCODE
    # Export the application only; dependency artifacts remain in shared cache.
    if ($cargoExit -eq 0 -and $CargoArgs[0] -in @('build', 'b')) {
        $artifactProfile = if ('--release' -in $CargoArgs) { 'release' } else { 'debug' }
        $profileFlag = [Array]::IndexOf($CargoArgs, '--profile')
        if ($profileFlag -ge 0 -and $profileFlag + 1 -lt $CargoArgs.Count) { $artifactProfile = $CargoArgs[$profileFlag + 1] }
        $application = Join-Path $env:CARGO_TARGET_DIR "$artifactProfile\tide.exe"
        if (Test-Path -LiteralPath $application) {
            $deliveryDirectory = Join-Path $repoRoot "target\$artifactProfile"
            New-Item -ItemType Directory -Path $deliveryDirectory -Force | Out-Null
            Copy-Item -LiteralPath $application -Destination (Join-Path $deliveryDirectory 'tide.exe') -Force
        }
    }
} finally { Pop-Location }
exit $cargoExit

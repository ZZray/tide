param([Parameter(Mandatory=$true)][string]$Installer)
$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$root = Join-Path $env:TEMP ('tide-installer-check-' + [guid]::NewGuid().ToString('N'))
$installation = Join-Path $root 'app'
New-Item -ItemType Directory -Path $root -Force | Out-Null
$process = Start-Process -FilePath $Installer -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER',"/DIR=`"$installation`"","/LOG=`"$root/install.log`"") -PassThru -Wait
if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
$binary = Join-Path $installation 'tide.exe'
if (!(Test-Path -LiteralPath $binary)) { throw 'Installed executable is missing.' }
foreach ($file in @('LICENSE','THIRD-PARTY-LICENSES.txt','README.zh-CN.md','licenses/rust/COPYRIGHT-library.html')) {
    if (!(Test-Path -LiteralPath (Join-Path $installation $file))) { throw "Installed notice/document missing: $file" }
}
$start = [Diagnostics.ProcessStartInfo]::new($binary)
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.ArgumentList.Add('--version')
$versionProcess = [Diagnostics.Process]::Start($start)
$stdout = $versionProcess.StandardOutput.ReadToEndAsync()
$stderr = $versionProcess.StandardError.ReadToEndAsync()
$versionProcess.WaitForExit()
$version = $stdout.GetAwaiter().GetResult().Trim()
$errors = $stderr.GetAwaiter().GetResult()
if ($versionProcess.ExitCode -ne 0 -or $version -notmatch '^tide 0\.' -or $errors) { throw 'Installed CLI version check failed.' }
$versionProcess.Dispose()
$uninstaller = Join-Path $installation 'unins000.exe'
$uninstall = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART') -PassThru -Wait
if ($uninstall.ExitCode -ne 0) { throw "Uninstaller failed: $($uninstall.ExitCode)" }
for ($attempt=0;$attempt -lt 20 -and (Test-Path -LiteralPath $binary);$attempt++) { Start-Sleep -Milliseconds 250 }
if (Test-Path -LiteralPath $binary) { throw 'Uninstaller left the executable behind.' }
Write-Output "Installer, installed CLI ($version), notices and uninstaller passed. Evidence: $root"

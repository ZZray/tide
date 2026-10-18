param([string]$Executable = (Join-Path $PSScriptRoot '..\target\release\tide.exe'))
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$smokeRoot = Join-Path (Split-Path $PSScriptRoot -Parent) ('target\smoke\' + [Guid]::NewGuid().ToString('N'))
$folderA = Join-Path $smokeRoot 'device-a'
$folderB = Join-Path $smokeRoot 'device-b'
$remote = Join-Path $smokeRoot 'remote.git'
$config = Join-Path $smokeRoot 'config.toml'
New-Item -ItemType Directory -Path $folderA,$folderB -Force | Out-Null
# System Git is used only to create the test fixture. Tide does its own Git I/O.
& git init --bare --quiet $remote
if ($LASTEXITCODE) { throw 'Could not initialize the local test remote.' }
function Invoke-Tide {
    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('--config', $config) + $args) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $output = $stdout.GetAwaiter().GetResult()
    $errorText = $stderr.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    if ($exitCode) { throw "Tide failed: $args; $errorText" }
    $output
}
Invoke-Tide init
Invoke-Tide add device-a $folderA $remote
Invoke-Tide add device-b $folderB $remote
$document = 'note.bin'
[IO.File]::WriteAllBytes((Join-Path $folderA $document), [byte[]](0,1,13,10,255))
Start-Sleep -Milliseconds 2100
Invoke-Tide sync device-a
Invoke-Tide sync device-b
$originalHash = (Get-FileHash -LiteralPath (Join-Path $folderA $document)).Hash
if ((Get-FileHash -LiteralPath (Join-Path $folderB $document)).Hash -ne $originalHash) { throw 'Original bytes differ.' }
[IO.File]::WriteAllBytes((Join-Path $folderA $document), [byte[]](0,65,13,10,255))
[IO.File]::WriteAllBytes((Join-Path $folderB $document), [byte[]](0,66,13,10,255))
Start-Sleep -Milliseconds 2100
Invoke-Tide sync device-a
$result = (Invoke-Tide sync device-b) -join "`n" | ConvertFrom-Json
if ($result.report.conflicts.Count -ne 1) { throw 'Expected one binary conflict.' }
$copy = Join-Path $folderB $result.report.conflicts[0].copy
if (-not (Test-Path -LiteralPath $copy)) { throw 'Conflict copy missing.' }
if ([IO.File]::ReadAllBytes($copy)[1] -ne 66) { throw 'Local conflict version was not retained.' }
Invoke-Tide sync device-a
if ((Get-FileHash -LiteralPath (Join-Path $folderA $document)).Hash -ne (Get-FileHash -LiteralPath (Join-Path $folderB $document)).Hash) { throw 'Devices did not converge.' }
$evidence = [ordered]@{
    success = $true
    executable = $Executable
    sha256 = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash
    run_directory = $smokeRoot
    conflict_copy = $result.report.conflicts[0].copy
    checks = @('exact binary bytes', 'two-device synchronization', 'both conflict versions retained', 'convergence')
}
$evidence | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $smokeRoot 'evidence.json') -Encoding utf8
$evidence | ConvertTo-Json

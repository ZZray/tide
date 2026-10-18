param(
    [Parameter(Mandatory = $true)][string]$Remote,
    [string]$Branch = 'tide/test',
    [string]$Executable = (Join-Path $PSScriptRoot '..\target\release\tide.exe')
)
$ErrorActionPreference = 'Stop'
$env:GIT_TERMINAL_PROMPT = '0'
$env:GCM_INTERACTIVE = 'never'
if ($Branch -eq 'main' -or $Branch -eq 'master') { throw 'Choose a dedicated test branch.' }
if ($Remote -match '^https?://[^/]*@') { throw 'Credentials must not be embedded in Remote.' }
& git check-ref-format --branch $Branch | Out-Null
if ($LASTEXITCODE) { throw 'Invalid test branch.' }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$runName = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0,8)
$runRoot = Join-Path (Split-Path $PSScriptRoot -Parent) ('target\real-remote\' + $runName)
$a = Join-Path $runRoot 'device-a'
$b = Join-Path $runRoot 'device-b'
$configA = Join-Path $runRoot 'a.toml'
$configB = Join-Path $runRoot 'b.toml'
New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
function Invoke-Git { & git @args; if ($LASTEXITCODE) { throw "Git operation failed: $($args[0])" } }
function Invoke-Tide([string]$ConfigPath, [string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('--config', $ConfigPath) + $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $output = $stdout.GetAwaiter().GetResult()
    $errorText = $stderr.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    if ($exitCode) { Write-Host $output; Write-Host $errorText; throw "Tide operation failed: $($Arguments[0]); see $runRoot" }
    $output
}
$mainBefore = (Invoke-Git ls-remote --heads $Remote refs/heads/main) -join ''
$existing = (Invoke-Git ls-remote --heads $Remote "refs/heads/$Branch") -join ''
if ($existing) { Invoke-Git clone --quiet --single-branch --branch $Branch $Remote $a }
else {
    Invoke-Git clone --quiet --single-branch --branch main $Remote $a
    Invoke-Git -C $a switch -c $Branch
    # Create only the authorized test ref, at the existing main revision.
    Invoke-Git -C $a push origin "HEAD:refs/heads/$Branch"
}
Invoke-Git clone --quiet --single-branch --branch $Branch $Remote $b
foreach ($entry in @(@($configA,$a,'TideTest-A'),@($configB,$b,'TideTest-B'))) {
    Invoke-Tide $entry[0] @('init') | Out-Null
    Invoke-Tide $entry[0] @('add','test',$entry[1],$Remote,'--branch',$Branch) | Out-Null
    $text = [IO.File]::ReadAllText($entry[0]).Replace('interval = "15m"','interval = "0s"').Replace('on_change = true','on_change = false')
    $text = [regex]::Replace($text,'(?m)^device_name = .*$',('device_name = "'+$entry[2]+'"'))
    [IO.File]::WriteAllText($entry[0],$text,[Text.UTF8Encoding]::new($false))
}
$relative = "sync-validation/$runName/note.bin"
$fileA = Join-Path $a $relative
$fileB = Join-Path $b $relative
New-Item -ItemType Directory -Path (Split-Path $fileA -Parent) -Force | Out-Null
[IO.File]::WriteAllBytes($fileA,[byte[]](0,1,13,10,255))
Start-Sleep -Milliseconds 2100
$first = ((Invoke-Tide $configA @('sync','test')) -join "`n") | ConvertFrom-Json
Invoke-Tide $configB @('sync','test') | Out-Null
if ((Get-FileHash -LiteralPath $fileA).Hash -ne (Get-FileHash -LiteralPath $fileB).Hash) { throw 'Initial bytes mismatch.' }
[IO.File]::WriteAllBytes($fileA,[byte[]](0,65,13,10,255))
[IO.File]::WriteAllBytes($fileB,[byte[]](0,66,13,10,255))
Start-Sleep -Milliseconds 2100
Invoke-Tide $configA @('sync','test') | Out-Null
$merge = ((Invoke-Tide $configB @('sync','test')) -join "`n") | ConvertFrom-Json
if ($merge.report.conflicts.Count -ne 1) { throw 'Expected exactly one binary conflict.' }
$copy = Join-Path $b $merge.report.conflicts[0].copy
if ([IO.File]::ReadAllBytes($copy)[1] -ne 66) { throw 'Local conflict bytes were not retained.' }
Invoke-Tide $configA @('sync','test') | Out-Null
Invoke-Tide $configB @('restore','test',$first.report.head,'--path',$relative) | Out-Null
Invoke-Tide $configB @('sync','test') | Out-Null
Invoke-Tide $configA @('sync','test') | Out-Null
if ([IO.File]::ReadAllBytes($fileA)[1] -ne 1) { throw 'Historical restore did not round-trip.' }
$mainAfter = (Invoke-Git ls-remote --heads $Remote refs/heads/main) -join ''
if ($mainBefore -ne $mainAfter) { throw 'Remote main changed during the test; investigate external activity.' }
$result = [ordered]@{
    success=$true; remote=$Remote; branch=$Branch; main_before=$mainBefore; main_after=$mainAfter
    binary_sha256=(Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash
    first_commit=$first.report.head; merged_commit=$merge.report.head; run_directory=$runRoot
    test_path=$relative; conflict_copy=$merge.report.conflicts[0].copy
    checks=@('HTTPS authentication','upload/download exact bytes','both conflict versions retained','history restore','main ref unchanged')
}
$result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $runRoot 'evidence.json') -Encoding utf8
$result | ConvertTo-Json -Depth 6

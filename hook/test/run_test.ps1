# Builds a fake NMS.exe in a throwaway game folder and runs it with the hook
# loaded, recorded by nmscheck's own listener.
# Usage: run_test.ps1 [-Crash] [-Long] [-Probe]
#
# -Probe turns on the metadata probe and runs it against the fake table in
# fake_nms.cpp. That table is the positive control: if the probe cannot find a
# table it was built to find, a negative result from the real game means nothing.
#
# Nothing here touches the real game, the real save folder, or the pipe the real
# game would use while it is running -- but note the listener does take the
# hook's pipe name, so do not run this while playing.
param([switch]$Crash, [switch]$Long, [switch]$Probe)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
$fake = Join-Path $env:TEMP 'NMSLoggerTest\No Man''s Sky'
$bin = Join-Path $fake 'Binaries'
New-Item -ItemType Directory -Force $bin, "$fake\GAMEDATA\MODS\TestMod\METADATA", "$fake\GAMEDATA\MODS\UnusedMod\GLOBALS" | Out-Null
Set-Content "$fake\GAMEDATA\MODS\TestMod\METADATA\REAL.MBIN" 'x'
Set-Content "$fake\GAMEDATA\MODS\TestMod\METADATA\SLASH.MBIN" 'x'
Set-Content "$fake\GAMEDATA\MODS\TestMod\METADATA\RELATIVE.MBIN" 'x'
Set-Content "$fake\GAMEDATA\MODS\UnusedMod\GLOBALS\GCDEBUGOPTIONS.GLOBAL.MBIN" 'x'

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -property installationPath
$env:PATH = "$(Split-Path $vswhere);$env:PATH"
$ErrorActionPreference = 'Continue'
cmd /c "call `"$vs\VC\Auxiliary\Build\vcvars64.bat`" >nul 2>&1 && cl /nologo /EHa /MT `"$PSScriptRoot\fake_nms.cpp`" /Fo`"$env:TEMP\\`" /Fe`"$bin\NMS.exe`" /link `"$root\hook\obj\xinput9_1_0.lib`" user32.lib shell32.lib 2>&1"
if ($LASTEXITCODE -ne 0) { throw 'fake NMS build failed' }
$ErrorActionPreference = 'Stop'

# The host is nmscheck now. Its session_dump example, in listen mode, records
# whatever connects on the pipe the DLL is built for -- which is the same code
# path the app itself uses. Build it first:
#     cd app\src-tauri; cargo build --example session_dump
$listener = "$root\app\src-tauri\target\debug\examples\session_dump.exe"
if (-not (Test-Path $listener)) {
    throw "Build the listener first:  cd app\src-tauri; cargo build --example session_dump"
}

# Install the hook by hand, rather than through the app: this fake install is not
# the one the app's settings point at.
Copy-Item "$root\hook\bin\xinput9_1_0.dll" $bin -Force
New-Item -ItemType Directory -Force "$bin\NMSLogger" | Out-Null

# Written explicitly rather than left to the DLL's first-run defaults, so that a
# previous -Probe run cannot leave the probe switched on for an ordinary one.
$ini = "$bin\NMSLogger\config.ini"
@"
[hook]
LogModFileOpens=1
LogFileFailures=1
LogFirstChanceExceptions=1
LogModuleLoads=1
LogSaveWrites=1
LogMemory=1
DebugOutputPerSecond=200
MetaProbe=$(if ($Probe) { 1 } else { 0 })
MetaProbeDelaySeconds=3
MetaProbeScanHeap=1
MetaProbeHeapBudgetMB=512
"@ | Set-Content $ini -Encoding ascii   # ASCII, not utf8: a BOM in front of
# [hook] makes GetPrivateProfileIntW miss the section entirely and every setting
# silently falls back to its default -- which looks exactly like a broken probe.
Remove-Item "$bin\NMSLogger\metaprobe_*.txt" -ErrorAction SilentlyContinue

$out = Join-Path $env:TEMP 'NMSLoggerTest\session.txt'
Remove-Item $out -ErrorAction SilentlyContinue
$watcher = Start-Process $listener -ArgumentList '--listen' -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $out
Start-Sleep 2

$p = if ($Crash) { Start-Process "$bin\NMS.exe" -ArgumentList 'crash' -PassThru -Wait }
     elseif ($Long) { Start-Process "$bin\NMS.exe" -ArgumentList 'long' -PassThru -Wait }
     elseif ($Probe) { Start-Process "$bin\NMS.exe" -ArgumentList 'probe' -PassThru -Wait }
     else { Start-Process "$bin\NMS.exe" -PassThru -Wait }
"fake game exit code: $($p.ExitCode)"

Start-Sleep 6
Stop-Process $watcher -ErrorAction SilentlyContinue
Get-Content $out

if ($Probe) {
    $reports = Get-ChildItem "$bin\NMSLogger\metaprobe_*.txt" -ErrorAction SilentlyContinue
    if (-not $reports) { throw 'the probe wrote no report -- it did not run' }
    foreach ($r in $reports) {
        Write-Host ""
        Write-Host "=== $($r.FullName) ==="
        Get-Content $r.FullName
    }
    # The control has to pass, or a negative result from the real game proves nothing.
    $text = Get-Content $reports[0].FullName -Raw
    if ($text -notmatch 'VERDICT: WALKABLE') {
        throw 'the probe missed its own positive control (see the report above)'
    }
    if ($text -notmatch 'MEMBER TABLE: record size 0x10') {
        throw 'the probe found a table but not the 0x10 member records it was given'
    }
    Write-Host ""
    Write-Host "positive control PASSED: the probe found the fake table and its member names."
}

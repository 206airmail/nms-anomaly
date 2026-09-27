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
InstProbe=$(if ($Probe) { 1 } else { 0 })
InstProbeBudgetMB=512
StringHunt=$(if ($Probe) { 1 } else { 0 })
"@ | Set-Content $ini -Encoding ascii   # ASCII, not utf8: a BOM in front of
# [hook] makes GetPrivateProfileIntW miss the section entirely and every setting
# silently falls back to its default -- which looks exactly like a broken probe.
Remove-Item "$bin\NMSLogger\metaprobe_*.txt" -ErrorAction SilentlyContinue
Remove-Item "$bin\NMSLogger\instprobe_*.txt" -ErrorAction SilentlyContinue
Remove-Item "$bin\NMSLogger\stringhunt_*.txt" -ErrorAction SilentlyContinue

# One id from the fake pool (0x10 stride) and one from the fake inventory (0x30).
# The hunt's whole job is to tell those apart, so the control names both.
@"
# written by run_test.ps1
ZZPOOL3
FERRITE_DUST
"@ | Set-Content "$bin\NMSLogger\probe_strings.txt" -Encoding ascii

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
    Write-Host "metadata control PASSED: the probe found the fake table and its member names."

    # The instance probe, against the synthetic inventory in fake_nms.cpp. Without
    # this, a "no inventory found" from the real game cannot be told apart from a
    # broken probe.
    $inst = Get-ChildItem "$bin\NMSLogger\instprobe_*.txt" -ErrorAction SilentlyContinue
    if (-not $inst) { throw 'the instance probe wrote no report -- it did not run' }
    Write-Host ""
    Write-Host "=== $($inst[0].FullName) ==="
    Get-Content $inst[0].FullName
    $itext = Get-Content $inst[0].FullName -Raw

    # Phase P: the run of 27. Asserted on the COUNT, not on "a run was found" --
    # a cluster of three would satisfy any presence check while meaning the stride
    # or the handle shape is wrong.
    if ($itext -notmatch 'VERDICT: CONTAINER RUN FOUND \(spans 27 of 27 strides') {
        throw 'phase P did not find the full 27-stride run in its own control'
    }
    if ($itext -notmatch 'The full run: the cluster spans all 27 strides') {
        throw 'phase P spanned 27 strides but did not recognise the run as complete'
    }
    # 25, not 27: the fixture leaves two containers empty on purpose, because an
    # unowned storage chest is empty in a real save too. The number is asserted
    # exactly -- 27 would mean the empty ones were wrongly counted, and fewer than
    # 25 would mean populated ones were dropped.
    if ($itext -notmatch '25 populated containers spanning 27 strides') {
        throw 'phase P did not report exactly 25 populated containers across the run'
    }
    # The lying grid values must not have excluded their containers.
    foreach ($lie in @('width=1 height=1', 'width=16 height=1')) {
        if ($itext -notmatch [regex]::Escape($lie)) {
            throw "phase P dropped the container with $lie -- Width/Height are not a filter"
        }
    }
    Write-Host ""
    Write-Host "phase P control PASSED: full 27-run found, lying grid values kept."

    if ($itext -notmatch 'VERDICT: (INVENTORY FOUND|CONTAINER RUN FOUND)') {
        throw 'the instance probe missed its own positive control (report above)'
    }
    # Phase P now short-circuits before the container-signature phase, so the
    # assertions that belonged to that phase only apply when it actually ran.
    # They are kept rather than deleted: the signature path is still the fallback
    # when no run is found, and an untested fallback is not a fallback.
    if ($itext -match 'PHASE 1 -- containers by their own signature') {
        $nContainers = ([regex]::Matches($itext, '(?m)^  CONTAINER at ')).Count
        if ($nContainers -gt 0) {
            if ($itext -notmatch 'FakeFreighterStorage4') {
                throw 'the container-signature phase found containers but not the fake one'
            }
            if ($itext -notmatch 'width=10 height=1 version=4 class=3 stackGroup=1 isCool=1') {
                throw 'the container fields did not read back exactly as the fake wrote them'
            }
        }
    }
    # Item ids must come back off the run, whichever phase reported it.
    foreach ($needle in @('FUEL1', 'ASTEROID1')) {
        if ($itext -notmatch $needle) { throw "the instance probe did not report $needle" }
    }
    Write-Host ""
    Write-Host "instance control PASSED: container, slot count and item ids all recovered."

    # The string hunt. Its question is not "is the id text" -- run 1 answered that
    # yes and it turned out to mean nothing -- but "what structure is the text IN".
    $sh = Get-ChildItem "$bin\NMSLogger\stringhunt_*.txt" -ErrorAction SilentlyContinue
    if (-not $sh) { throw 'the string hunt wrote no report -- it did not run' }
    Write-Host ""
    Write-Host "=== $($sh[0].FullName) ==="
    Get-Content $sh[0].FullName
    $htext = Get-Content $sh[0].FullName -Raw

    # Classification, both ways round. A hunt that called the pool an element array
    # (or the reverse) would still find every string and still look like it worked.
    if ($htext -notmatch 'ZZPOOL3\s+stride 0x10\s+id pool') {
        throw 'the hunt did not recognise the 0x10 id pool for what it is'
    }
    if ($htext -notmatch 'FERRITE_DUST\s+stride 0x30\s+\*\*\* element stride') {
        throw 'the hunt did not classify the fake inventory slot at the element stride'
    }
    if ($htext -notmatch 'VERDICT: ELEMENT-STRIDE HITS FOUND') {
        throw 'the hunt found an element-stride hit but did not say so in the verdict'
    }

    # And the count, because this is where run 1 actually went wrong: two of its six
    # RED2 hits were the probe's own copy of the id list, sitting on the probe
    # thread's stack. The fake pool holds ZZPOOL3 exactly once, so more than one
    # heap hit means the hunt is finding itself again.
    $nPool = ([regex]::Matches($htext, '(?m)^\s+0x[0-9A-F]+\s+ZZPOOL3\s')).Count
    if ($nPool -ne 1) {
        throw "the hunt reported $nPool heap hits for ZZPOOL3; the fake pool holds exactly 1 " +
              "(more than that means it is finding its own copy of the list again)"
    }
    Write-Host ""
    Write-Host "string hunt control PASSED: pool and element stride told apart, no self-hits."
}

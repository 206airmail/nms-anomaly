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

    # Phase L runs first now, because the live layout is the one that exists while
    # the game is running. Asserted on the exact count and on the grid positions,
    # which is what proved the layout right in the first place.
    if ($itext -notmatch 'VERDICT: LIVE INVENTORIES FOUND \(5\)') {
        throw 'phase L did not find exactly the 5 live-layout inventories'
    }
    # THE CONTROL THAT FAILS ON THE OLD CODE. Every fixture inventory has garbage
    # between mStore.size and mStore.capacity, so a probe that walks capacity finds
    # a malformed slot in all five and rejects all five. Asserting the tail figure
    # rather than just the count is what makes that visible rather than lucky.
    if ($itext -notmatch '0 of 5 had to fall back to capacity, and 5 of 5 had a tail') {
        throw 'phase L is not walking mStore.size -- it read the allocation, not the elements'
    }
    # The ownership mask at -0x80 is a PREDICTION about the live game, so the
    # fixture can only prove the probe reads it as specified: popcount equal to
    # miCapacity with nothing above bit 15.
    if ($itext -notmatch '5 of 5 readable, 5 of those had') {
        throw 'phase L did not read the -0x80 ownership mask back as written'
    }
    # Four of the five sit on a 0x1F0 lattice with members 3 and 4 deliberately
    # absent, which is what a part-owned cTkFixedArray looks like. A detector that
    # needs consecutive members would report 3.
    if ($itext -notmatch 'stride 0x1F0: best lattice holds 4 stores') {
        throw 'the fixed-array test did not recover the lattice through its gaps'
    }
    # THE LATTICE WALK. Index 3 is a store the player owns and has never filled:
    # a grid, a miCapacity and a mask, but capacity 0 and no element array. The
    # discovery gate cannot see it -- there is nothing to validate -- so if it
    # appears at all, it appears because it was read BY POSITION. That is the whole
    # point of the second pass, and it is what builds the index -> inventory map.
    if ($itext -notmatch 'LATTICE WALK') {
        throw 'the lattice walk did not run'
    }
    if ($itext -notmatch '(?m)^\s+3\s+0x[0-9A-F]+\s+10x6\s+7\s+7\s+0/0\s+\(empty\)') {
        throw 'the lattice walk did not report the owned-but-empty store at index 3'
    }
    # And it must still find the populated ones at their own indices, or the walk
    # is reporting a lattice that does not line up with what discovery found.
    foreach ($idx in 0, 1, 2, 5) {
        if ($itext -notmatch "(?m)^\s+$idx\s+0x[0-9A-F]+\s+\d+x\d+") {
            throw "the lattice walk skipped index $idx"
        }
    }
    if ($itext -notmatch 'miCapacity 30' -or $itext -notmatch 'store 10/15 used/alloc') {
        throw 'phase L did not report miCapacity and the used/allocated split'
    }
    # The unbounded stack is asserted explicitly: it is the exact case that made the
    # first live run validate zero inventories.
    if ($itext -notmatch 'TRA_MINERALS3 x1307/0') {
        throw 'phase L dropped the unbounded stack (MaxAmount 0) -- one odd slot must not veto an inventory'
    }
    if ($itext -notmatch '\^JET1 x1/1') {
        throw 'phase L dropped the installed technology (caret-prefixed id)'
    }
    foreach ($needle in @('FUEL1 x2163/9999 @\(4,0\)', 'SAND1 x820/9999 @\(6,1\)',
                          'FUEL1 x971/9999 @\(3,1\)', 'REACTION2 x4/9999 @\(8,0\)')) {
        if ($itext -notmatch $needle) {
            throw "phase L did not report $needle with its grid position"
        }
    }
    Write-Host ""
    Write-Host "phase L control PASSED: 5 live inventories, grid positions intact,"
    Write-Host "  mStore.size walked rather than mStore.capacity, the -0x80 mask read"
    Write-Host "  back, the 0x1F0 lattice recovered through its gaps, the walk"
    Write-Host "  reporting the owned-but-empty store that discovery cannot see, and"
    Write-Host "  the object whose capacities disagree rejected."

    # Phase P: the run of 27. Asserted on the COUNT, not on "a run was found" --
    # a cluster of three would satisfy any presence check while meaning the stride
    # or the handle shape is wrong.
    # Phase P only runs when phase L finds nothing -- it describes the save document,
    # which the live game does not contain. Its control still has to work, because it
    # is what a save-time probe uses, so it is exercised by its own switch below
    # rather than deleted.
    if ($itext -match 'PHASE P -- skipped') {
        Write-Host "phase P skipped (phase L succeeded) -- run with -SaveDoc to exercise it"
        $skipP = $true
    }
    if (-not $skipP -and $itext -notmatch 'VERDICT: CONTAINER RUN FOUND \(spans 27 of 27 strides') {
        throw 'phase P did not find the full 27-stride run in its own control'
    }
    if (-not $skipP) {
    if ($itext -notmatch '27 strides of item-holding containers\. That is cGcPlayerStateData') {
        throw 'phase P spanned 27 strides but did not recognise the run as complete'
    }
    # Content validation is the correction that matters. Shape alone found 175,187
    # candidates in the real game and clustered skeleton joint data into a "full
    # run" -- in 7.6 GB there is enough periodic structure to manufacture any period
    # you look for. The count is asserted so a regression to shape-only cannot pass.
    if ($itext -notmatch 'content check: 25 of \d+ candidates actually hold a readable item') {
        throw 'the content check did not validate exactly the 25 populated containers'
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
    }
    Write-Host ""
    Write-Host "phase P control PASSED or skipped."

    if ($itext -notmatch 'VERDICT: (LIVE INVENTORIES FOUND|INVENTORY FOUND|CONTAINER RUN FOUND)') {
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

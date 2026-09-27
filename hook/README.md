# The session recorder's hook DLL

The half of the session recorder that lives **inside** the game. C++17, MSVC,
x64, static CRT, no dependencies beyond MinHook (vendored in `third_party/`,
BSD-2). What it does and why is in
[`../docs/session-recording.md`](../docs/session-recording.md).

```
.\build.ps1                  # -> bin\xinput9_1_0.dll  (about 230 KB)
copy bin\xinput9_1_0.dll ..\tools\nmslogger\   # where the app installs from
```

Needs Visual Studio 2022 or newer with the C++ x64 tools. `build.ps1` finds them
through `vswhere`.

## What each file is

| file | what |
|---|---|
| `src/common.h` | shared declarations, the level enum, **the pipe name and the marker string** |
| `src/dllmain.cpp` | startup, `config.ini`, the order things are installed in |
| `src/proxy.cpp`, `src/exports.def` | forwarding the real XInput exports, plus `NMSLoggerHookVersion` |
| `src/hooks.cpp` | every hook, the file-path logic, save accounting, the heartbeat and the memory report |
| `src/crash.cpp` | the vectored handler and the crash report |
| `src/log.cpp` | the event queue, the writer thread, the raw log, the pipe client, the backlog |
| `src/util.cpp` | UTF-8, address → `module+offset`, the text classifier |
| `src/metaprobe.cpp` | **the one file that looks at the game's own memory** — see below |
| `test/fake_nms.cpp`, `test/run_test.ps1` | a fake game to exercise it against |

## config.ini

Written into `<game>/Binaries/NMSLogger/` on first run, and read at startup:
`LogModFileOpens`, `LogFileFailures`, `LogFirstChanceExceptions`,
`LogModuleLoads`, `LogSaveWrites`, `LogMemory`, `DebugOutputPerSecond`, and the
diagnostic `LogAllFileOpens` (very noisy: every successful open).

`LogSaveWrites` also decides whether `NtWriteFile` and `NtClose` are hooked at
all, so turning it off costs nothing on those paths.

The metadata probe adds `MetaProbe` (off by default), `MetaProbeDelaySeconds`,
`MetaProbeScanHeap`, `MetaProbeHeapBudgetMB` and `MetaProbeScanCode`.

Pass 1 costs (names x image bytes) and NMS.exe is a 118 MB image of which 52 MB
is `.text`, so executable regions are skipped by default -- a NUL-terminated
class name is not kept in code. `MetaProbeScanCode=1` puts them back, and that
is the first thing to try if the probe finds no names at all.

Every setting the hook actually ended up with is logged on one `config:` line at
startup. That is not decoration: `GetPrivateProfileIntW` silently returns *every*
default if the file has a UTF-8 BOM in front of `[hook]` or the section name is
misspelt, so a config that looks right and does nothing is otherwise impossible
to tell from a broken feature. Write `config.ini` as ASCII or UTF-16, never as
UTF-8-with-BOM — PowerShell's `Set-Content -Encoding utf8` emits a BOM.

## The metadata probe (`MetaProbe=1`)

One question, asked once: **does NMS carry a runtime table that names its own
classes and their members?** If it does, a read/write API for game state can be
generated from that table per build instead of maintained offset by offset, and
that changes the shape of any larger API project. If it does not, we learn so in
a session rather than a quarter.

How it reasons: the game must read `.MBIN` files, and an `.MBIN` names its
template rather than describing it, so the layout of every serialisable class has
to exist somewhere in the process. Pass 1 finds the class-name strings in
`NMS.exe`. Pass 2 finds every aligned qword in the process equal to one of those
addresses — each is a candidate descriptor. If hits for *different* classes sit
at a constant stride, that stride is the record size. Following a record's other
pointers one level down, an array whose records each begin with a pointer to a
short identifier is a member table, i.e. field names, and the answer is yes.

Results go to `NMSLogger\metaprobe_<n>.txt`, with a one-line verdict through the
normal log and pipe. It runs once after `MetaProbeDelaySeconds`, then again
whenever a file named `probe.now` appears in `NMSLogger\` — re-probe after a save
is loaded, because that is when the inventory containers actually exist.

**Search names come from `NMSLogger\probe_names.txt` when it exists** (one per
line, `#` comments), else a built-in list. A wrong name costs nothing but a "not
found" line, so prefer a long list generated from the real mod corpus over a
short list of good guesses.

### Four things to know before changing it

**Never let the probe hold a target address in memory it owns.** A buffer full of
pointers to class names *is* a descriptor table by every test here, so the probe
finds its own scratch space and reports it beautifully. Measured while building
this: 8192 phantom hits at a stride of 8 (a scratch array) and 362 at a stride of
0x20 (`sizeof(Hit)` in the results vector), both ranked above the real table.
`ScanForTargets` therefore records the target's *index*, `Hit` has no address
field, and the sorted target array and the probe thread stack are excluded by
range. Freed heap still holds stale copies, which is part of why `kMinStride`
exists — a real record holds a name pointer *and something else*, so an 8-byte
stride is noise by construction.

**Groups are ranked by coherence, not size.** One consistent name offset at a
stride of at least `kMinStride`, then distinct classes, then hit count. A big
noisy region always wins on volume and never on coherence. Known limitation: the
record boundary is taken from the lowest hit in a group, so a single stale hit
below the real table shifts every offset and the group reads "inconsistent". A
good group can land second for that reason — read the group list, not just the
verdict.

**Two safety layers, and both are needed.** Every address is checked against a
`VirtualQuery` map before it is touched, *and* every loop touching game memory is
SEH-guarded, because that map goes stale the moment the game frees something.
Functions containing `__try` touch PODs only and return results through
caller-provided arrays — an MSVC requirement, not a style choice.

**The probe thread sets `t_inHook`.** That is what stops our own file hooks
observing the report being written *and* stops the vectored handler in
`crash.cpp` logging the first-chance faults the probe provokes on purpose.

## The instance probe (`InstProbe=1`)

The metadata probe answers *what shape is a `cGcInventoryContainer`*. This one
answers *where is one right now*, which is a different problem and the only part
that needs a save loaded.

It is a signature match rather than a search, because the layout is known.
`cGcInventoryElement` is the anchor: its first field is a 16-byte NUL-padded ASCII
item id (`CARBON`, `^LAUNCHFUEL`), which is far too structured to occur by
accident, and the fields beside it constrain each other -- `Amount <= MaxAmount`,
`DamageFactor` in [0,1], and two fields that can only be 0 or 1. Three phases:
runs of elements; then one pass for pointers into those runs, since a dynamic-array
handle sits at `container+0x10`; then validation of the container itself.

Results go to `NMSLogger\instprobe_<n>.txt`. Set `MetaProbe=0` with
`MetaProbeDelaySeconds=0` and trigger with `probe.now` once in game, or the first
run fires at the main menu and reports nothing.

**Every offset in it is measured, and only for one build** -- they come from
`tools/nms_meta_extract.py` reading the descriptor table out of `NMS.exe`. After a
game patch, re-extract before trusting any of them.

### Three traps, all of which were hit while building it

**Growing a run can overshoot.** `GrowRun` extends backwards from its anchor
because the first slots of an inventory are usually empty, and it can run off the
front of the array into neighbouring heap. So phase B searches for *every* element
address in the run rather than the base it computed, and phase C believes the
container's own pointer over the one the probe derived.

**Reserve the target vector.** A growing vector of addresses leaves a copy of
itself in freed heap at every reallocation, and a run of pointers into the element
array is precisely what phase B hunts for -- so the probe finds its own discarded
scratch and calls each copy a container. Same failure the metadata probe had, in a
new disguise; see the note on `Hit` above.

**Bound the enums.** `Class` and `StackSizeGroup` are enums, `Version` is a small
save-format number, and a real container has at least one row and column. Without
those bounds the probe reported nine containers where there was one, and their
`Version` fields were the low halves of addresses.

### The positive control

`test\run_test.ps1 -Probe` runs both probes against synthetic data in
`fake_nms.cpp`: eight class records of a known size (0x28) naming real NMS classes
with member sub-tables of a known size (0x10), in both `.rdata` and the heap; and
one inventory container with ten slots, two of them deliberately empty, built byte
by byte at the measured offsets rather than as a C struct -- a compiler may pad a
struct differently, and then a passing test would prove nothing about the real
thing.

The script asserts that **exactly one** container comes back and that its fields
read out identically to what the fake wrote. That precision is the point: an
earlier version asserted only that `CARBON` appeared somewhere, and it passed
happily while the probe was reporting nine containers, eight of them junk.

This is not ceremony. Without it, "found nothing" from the real game is ambiguous
between *the game has no such table* and *the probe is broken*, which are the two
answers it most matters to tell apart.

## Two things to know before changing it

**The marker identifies the DLL, and the app relies on it.**
`NMSLOGGER_HOOK_MARKER_V1` in `common.h` is how `engine::hook` tells our DLL from
another tool's. Change it and every installed copy stops being recognised as
ours — which means the app will refuse to replace or remove it.

**The pipe name is the contract.** `NMSLOG_PIPE_NAME` must match
`engine::pipe::PIPE_NAME`. The protocol itself — one JSON object per line, a
`hello` first, `{ts, lvl, tid, cat, msg}` after — is documented in
`engine::sessionlog`, and a hook that sends a field the app has never heard of
still works: unknown fields are ignored and unknown `type` lines are skipped.

**Test against the fake game, not against a four-hour session.**
`test\run_test.ps1 [-Crash] [-Long] [-Probe]` exercises an absolute open, a
forward-slash relative open, an open relative to a directory handle (which is how
the game opens every mod file), a missing file, `FullLog` writes and their debug
echo, a handled access violation, and optionally a crash or a 65-second run for
the heartbeat.

Hard-won lessons about the hooking itself — why `CreateFileW` saw nothing, why
directory handles matter, the build flags — are in §6 of
`../../NMSLoggingTool/NMS_LOGGER_INTEGRATION.md`.

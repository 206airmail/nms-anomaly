# Recording what the game says while it runs

Every other part of nmscheck reasons about mods as files on disk. This part is
the only one that knows what the **game** did with them: which files it actually
opened, what it complained about, and where it crashed.

It exists because No Man's Sky says almost nothing. `GAMEDATA\FullLog.txt` is
usually one line, and the game reports nothing at all about mods — not which of
them it loaded, not which file won when two mods shipped the same asset, not
which of them was in the frame when it died.

Merged in from the standalone **NMS Logger** tool
(`../NMSLoggingTool/NMS_LOGGER_INTEGRATION.md` has the original write-up,
including everything that was learned about the game while building it). The
hook DLL is the same DLL; the C# host has been reimplemented in the Rust engine.

---

## The two halves

```
Steam launches NMS.exe
  │  Windows resolves NMS.exe's import of xinput9_1_0.dll from the program's
  │  own folder before System32
  ▼
xinput9_1_0.dll   (hook/, built to tools/nmslogger/)
  ├─ forwards every XInput call to the real System32 copy
  ├─ hooks ntdll!NtCreateFile / NtOpenFile, kernelbase!WriteFile,
  │  OutputDebugString, MessageBox, TerminateProcess, and the unhandled
  │  exception filter
  ├─ writes its own raw log to <game>\Binaries\NMSLogger\hook_latest.log
  └─ connects as a client to \\.\pipe\NMSLogger and sends one JSON line per event
                        ▼
nmscheck  (src/watch.rs + engine::{pipe, sessionlog, gameproc, hook, crashinfo})
  ├─ watches for NMS.exe, opens the pipe only while the game is up
  ├─ writes  %APPDATA%\com.nsbro.nmscheck\sessions\NMS_<when>.log
  ├─ writes  the same name with .json: the verdict and counts the list reads
  └─ on exit, adds the summary: exit code, crash report, mods that complained,
     files the game ignored, Windows' own crash record, dump files
```

The DLL is **not** installed by nmscheck on its own. The Sessions tab offers it,
says what it does, and removes it again; nothing is written into the game's
folder until the user presses the button.

## Why it is called `xinput9_1_0.dll`

`NMS.exe` imports XInput, and Windows resolves an import from the program's own
directory before System32. A copy there is loaded by the game with no injector,
no custom launcher, and no change to how the user starts the game.

It is also the slot other overlay tools reach for, which is why
`engine::hook` is careful: our DLL is recognised by a marker string compiled into
it (`NMSLOGGER_HOOK_MARKER_V1`), never by name, size or date. A foreign
`xinput9_1_0.dll` is never overwritten unless the user says so a second time, and
then it is copied to `xinput9_1_0.dll.nmscheck-backup` and put back on removal.
Nothing is written or deleted while the game is running, because Windows holds
the file open and a "successful" delete would leave the DLL live in the process.

## The lifecycle

`watch.rs` runs one thread for the life of the app:

1. look for `NMS.exe` every two seconds;
2. when it appears, open a handle to it — **before** it exits, because once the
   last handle closes the exit code is gone, and the exit code is how "it
   crashed" is known at all;
3. open the pipe and wait for the hook;
4. record until the pipe closes, batching events to the screen a few times a
   second;
5. wait up to 20 seconds for the process to actually exit, 3 more if the exit
   code looks like a crash (Windows Error Reporting files its event *after* the
   process is gone), then write the summary and prune old sessions;
6. go back to step 1.

The pipe is opened per run rather than held open, because it allows **one**
instance: holding it for the life of the app would stop any other copy of this
program — or the standalone logger, if someone still has it — from ever
recording, including while we are not interested.

A watchdog thread nudges the pipe when the game goes away, so a run where the
hook never connects (it is not installed, say) does not leave a thread waiting
for a client that will never come.

## Three things beyond the mods

Added after the first version, because the mods are not the only thing that can
ruin an evening.

### Saves

**In the hook.** Every write to a file under `HelloGames/NMS` ending `.hg` is
accounted for, and reported when the game *closes* the file -- which is the moment
a save is actually finished: `wrote save3.hg: 786432 bytes in 12 writes over 380
ms`. A write that fails is reported at Error with its Windows error code (112 is a
full disk), and a save the game never closed is reported as unfinished on the way
out. A save in flight when the game crashed is named in the crash report, before
the file list, because it is the one line there that changes what the player
should do next.

Accounting happens at `NtWriteFile`, not at `WriteFile`: the latter calls the
former, so counting both would count every byte twice.

**In the app.** A failed or unfinished save leads the session verdict, ahead of a
crash -- a crash costs the session, a save costs the save, and only one of those
comes back by playing again.

### Copies of the save

`engine::savewatch`, and it needs **no hook at all**: it watches the save folder
and copies whatever changed. The hook, when installed, makes it prompt rather than
periodic -- a copy is taken the moment the game finishes writing one.

- A copy is per **slot**, both files together (`save3.hg` and its `mf_save3.hg`
  manifest). Mixing one moment's data with another's manifest would be worse than
  having no copy at all.
- A file touched in the last two seconds is left for the next pass. A torn copy
  that looks like a backup is the one outcome worse than none.
- Each copy carries a `taken.json` noting the save's own modification time. The
  folder name is for people to read; the note is what "has this changed since the
  last copy" is decided from, and it is never restored into the save folder.
- A restore copies aside what it replaces, so restoring the wrong moment is undone
  by restoring the copy it made. It refuses while the game is running, because the
  game holds the save in memory and would write over anything put back.

Measured on a real install: a whole save folder is 1.2 MB across ten files -- the
game compresses its saves -- so the default of ten copies per slot costs about
twelve megabytes.

### Memory, handles, and what changed

**Memory** rides the existing heartbeat timer -- 60 seconds in, then every five
minutes -- as a separate `memory` line of `key=value` pairs in bytes: working set,
peak, private, page faults, handles, and what the machine had free. The summary
reports the first sample against the last, which is the answer to "it gets worse
the longer I play".

**What changed** is `engine::machine`: a short description of what the game ran on
-- GPU and driver version, Windows build, the game's Steam build id, the enabled
mods in load order, free disk space -- kept with each session and compared against
the previous one. The log header and the session sheet then open with *"since your
last recorded session: the graphics driver changed 566.36 to 571.96; 1 mod
added"*. It is a suspect list, not a diagnosis, and a value that could not be read
reports nothing rather than reporting a change.

## What the summary is careful about

These were all wrong first, and the tests name each one:

- **The game says everything twice.** Every line it logs goes to `FullLog.txt`
  *and* through `OutputDebugString`. `sessionlog::Echo` keeps whichever copy
  arrives first and drops the other; without it every count doubles.
- **The game asks for mod paths upper-cased.** It opens
  `MODS\TERRALYSIS_PATH_OF_ORION\…` for a folder called
  `TERRALYSIS_Path_Of_Orion`. Attributed by what the game said, every file it
  opened belonged to a mod nobody has, and the real mod was reported as having
  loaded nothing.
- **A mod named in a crash report did not cause the crash.** The report lists the
  last 48 files opened as context. Attribution skips the `crash`, `module` and
  `hook` categories.
- **"Never opened" is three findings, not one.** A file whose other-format twin
  *was* opened is definitely dead weight. A mod with no loadable files at all
  cannot do anything. Everything else may simply be content the session never
  reached — situational layouts, a texture for a biome nobody visited — and is
  reported separately and cautiously.

## Reading and rebuilding

- `cargo run --example session_dump` plays a scripted session down a real named
  pipe (its own name, so it cannot displace whatever is recording the game) and
  prints the log and the verdict. It is the end-to-end check for the plumbing
  the unit tests cannot reach.
- `hook/test/run_test.ps1 [-Crash] [-Long]` builds a fake `NMS.exe`, loads the
  real DLL into it, and records it with `session_dump --listen`. It exercises the
  save hooks, and with `-Long` the memory heartbeat. Do not run it while playing:
  the listener takes the hook's own pipe name.
- `hook/build.ps1` rebuilds the DLL; see `hook/README.md`. The built DLL belongs
  at `tools/nmslogger/xinput9_1_0.dll`, which is where the app looks for it and
  what `tauri.conf.json` bundles. `NMSCHECK_HOOK_DLL` points at a different one.
- The pipe name is compiled into both halves (`hook/src/common.h` and
  `engine::pipe::PIPE_NAME`). Changing it means rebuilding the DLL.

## Where things are kept

| what | where |
|---|---|
| session logs and their briefs | `%APPDATA%\com.nsbro.nmscheck\sessions\` |
| the hook's own raw log, and its `config.ini` | `<game>\Binaries\NMSLogger\` |
| copies of the save | `%APPDATA%\com.nsbro.nmscheck\save-backups\` |
| the game's own saves | `%APPDATA%\HelloGames\NMS\<account>\save*.hg` |
| the installed hook | `<game>\Binaries\xinput9_1_0.dll` |
| the DLL we install from | `tools/nmslogger/xinput9_1_0.dll` |

How many sessions are kept is a setting (`keep_sessions`, 25 by default); the
oldest go when a new one finishes. Recording can be switched off entirely in
Settings, and costs nothing while the game is closed either way. Copies of the
save have their own switch (`backup_saves`) and their own limit
(`keep_save_backups`, ten per slot).

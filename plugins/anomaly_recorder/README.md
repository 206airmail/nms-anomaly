# Anomaly session recorder

An [Atlas](https://github.com/206airmail/nms-atlas) plugin. Everything Anomaly
wants from inside the running game — which files the game opened, which saves
it wrote, how much memory it used, and why it crashed.

This used to be the whole `xinput9_1_0.dll`. It is now a plugin, because only
one mod can own that filename and Atlas has to be the one in it.

## What did not change

Deliberately, so the Anomaly desktop app needs no changes on its side:

- reads `<game>\Binaries\NMSLogger\config.ini`, section `[hook]`
- writes `<game>\Binaries\NMSLogger\hook_latest.log`
- speaks the same JSON protocol over the same named pipe (`\\.\pipe\NMSLogger`)
- reports the same marker, so the host still recognises it

## What did change, and it is a real tradeoff

**Hooks install later than they used to.** As a proxy DLL this code ran in
`DllMain`, before the game had opened a single file. As a plugin it runs when
Atlas loads it, which is after the game is up.

So **a small number of the earliest file opens are no longer recorded**. That
is permanent, not a bug to be fixed, and it is the price of the plugin
architecture. It does not affect what Anomaly actually uses the recorder for —
which mod files loaded, which saves were written, and crashes — because all of
that happens long after startup. Module loads are explicitly caught up when the
hooks install, so the module list stays complete.

If very-early file opens ever matter, the answer is a capability in Atlas
(which *does* run in `DllMain`) rather than moving this back into the proxy
slot.

## Why it bundles MinHook

Atlas patches no game code and offers no hooking capability, so a plugin that
wants to detour `CreateFileW` brings its own detour library. MinHook is
referenced from `hook/third_party/minhook` rather than copied, so there is one
vendored version in the repo to keep current.

Note the gap this implies: **two plugins hooking the same function have nobody
arbitrating between them.** Atlas may grow a hosted hooking capability later;
because capabilities are discovered by export name, adding one will not break
anything built before it.

## Building

    .\build.ps1

Then copy `bin\anomaly_recorder.dll` into `<game>\Binaries\Atlas\plugins\`.

## What is not here

`metaprobe.cpp` — the metadata/instance probe — stayed in `hook/` as a research
tool. It is not a plugin and is not needed at runtime. The one place the
recorder used to poke it (on a save being written) is now a no-op.

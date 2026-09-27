# Packaging nmscheck for distribution

```
cd app && npx tauri build
```

Produces, in `app/src-tauri/target/release/bundle/`:

| bundle | size |
|---|---|
| `msi/nmscheck_<ver>_x64_en-US.msi` | ~12.6 MB |
| `nsis/nmscheck_<ver>_x64-setup.exe` | ~11.7 MB |

## What ships

Verified by extracting the MSI (`msiexec /a`), not by guessing:

```
app.exe                     4.8 MB   the Rust engine + the compiled UI
tools/hgpaktool.exe        10.1 MB   extracts vanilla assets from PCBANKS
tools/MBINCompiler.exe      2.4 MB   decompiles .MBIN into readable XML
tools/nmslogger/
  xinput9_1_0.dll           239 KB   the session recorder, installed by the user
tools/THIRD-PARTY.md                 attribution and licences
```

That is the whole payload. The figures above predate the recorder; re-extract
after the next build rather than trusting the arithmetic.

**The recorder ships but is not installed.** It sits in `tools/` like the other
two and only reaches the game when the user presses Install on the Sessions tab
— see `session-recording.md`. Ours is the only build the app will replace or
remove, which it tells apart by a marker string inside the file, so a packaged
copy that differs from the installed one is offered as an update rather than
written over silently.

## What does not ship, and must not start

`nmscc/` — the Python engine — is **not** part of the product. It is retained
in the repository only as an independent cross-check: `tools/compare_engines.py`
runs both engines against the live library and diffs them, which is how a
mistake in the Rust shows up as a failing check rather than a wrong answer in
the UI. Nothing in the app calls it, and nothing in the bundle contains it.

Also absent, and expected to stay absent: `tools/venv/`, `tests/`, `docs/`,
`backups/`, `hook/` — the recorder's C++ source, of which only the built DLL
ships — and any `.py` file.

Tauri bundles only the frontend build output plus what `bundle.resources` in
`app/src-tauri/tauri.conf.json` names, so this is the default rather than
something enforced. If that list grows, re-check it — the MSI extraction above
takes a minute and is the only way to be sure.

## The two binaries

Neither is built by this project. See `tools/THIRD-PARTY.md` for sources,
authors and licences (MBINCompiler is LGPL-3.0, hgpaktool is MIT) and for how a
user substitutes their own build.

**`tools/hgpaktool.exe` is not the one pip installs.** The pip launcher hard-codes
an absolute path to the build machine's `python.exe`, so it runs on exactly one
computer. Rebuild the redistributable copy with:

```
tools/venv/Scripts/python.exe tools/build_hgpaktool.py
```

It freezes the package with PyInstaller and refuses to install a binary that
still references the checkout. Re-run it after upgrading hgpaktool.

`tools/MBINCompiler.exe` is upstream's own release, self-contained, copied as-is.

## Why a missing tool is not allowed to be quiet

Without these binaries the scene check cannot run, and a scene check that
cannot run produces no findings — which is indistinguishable from a clean
library. Every report therefore carries a `tools` block:

```json
"tools": {
  "mbincompiler": "...\\tools\\MBINCompiler.exe",
  "hgpaktool":    "...\\tools\\hgpaktool.exe",
  "scene_check": true,
  "scene_check_note": null
}
```

When `scene_check` is false, `scene_check_note` says which piece was missing and
the UI shows "Scene files were **not checked**" instead of a verdict. A packaged
build that failed to find its own resources would be visible immediately rather
than silently downgraded — which is what happened before any of this existed.

Tool discovery order, first hit wins:

1. `NMS_MBINCOMPILER` / `NMS_HGPAKTOOL`, honoured exactly — a path that does not
   exist is an error, never a silent fall back
2. the resource directory of the running app (a packaged build)
3. `tools/` beside the executable, then the executable's own directory
4. the checkout's `tools/`, for `cargo run` during development
5. `PATH`

Ordering matters: a shipped build must prefer what shipped with it over a path
compiled in from the machine that built it. `engine/tools.rs` pins that with a
test.

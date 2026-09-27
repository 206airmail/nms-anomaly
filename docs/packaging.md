# Packaging Anomaly for distribution

```
cd app && npx tauri build
```

Produces, in `app/src-tauri/target/release/bundle/`:

| bundle | size |
|---|---|
| `nsis/Anomaly_<ver>_x64-setup.exe` | 13.9 MB |
| `msi/Anomaly_<ver>_x64_en-US.msi` | 16.2 MB |

**Delete stale bundles before publishing.** The directory is not cleaned between
builds, so the rename from `nmscheck` once left a full set of `nmscheck_*`
installers sitting beside the new ones, indistinguishable from a current release.

## What ships

Verified 2026-09-27 by **extracting** the NSIS installer, not by reading the
config and not by listing it either — a listing showed `License.txt` with a blank
size column, which had to be extracted to prove it was not a zero-byte file.

```
anomaly.exe                       8.6 MB   the Rust engine + the compiled UI
tools/hgpaktool.exe               9.7 MB   extracts vanilla assets from PCBANKS
tools/MBINCompiler.exe            2.2 MB   decompiles .MBIN into readable XML
tools/sevenzip/
  7z.exe                          562 KB   extracts downloaded mod archives
  7z.dll                          1.8 MB   the formats beyond .7z live here
  License.txt                    6031 B    upstream's own terms
tools/nmslogger/
  xinput9_1_0.dll                 343 KB   the session recorder, user-installed
tools/THIRD-PARTY.md             4903 B    attribution and licences
```

Re-extract after any build rather than trusting the arithmetic above — these
figures have been wrong before, and once the *contents* were wrong too.

## Why 7-Zip is bundled rather than required

Nexus mods for this game arrive as `.zip`, `.7z` and `.rar`. RAR's licence forbids
using its reference code to *create* an implementation, so a pure-Rust reader for
all three is not something to reach for casually. 7-Zip reads them all and is
redistributable.

`7z.exe` alone handles little beyond the 7z format; `7z.dll` is what adds the rest,
so the two ship together and `engine::archive::find_7z` looks for them together.

Lookup order: `NMSCHECK_7Z` if set, then the bundled copy, then an installed 7-Zip
under Program Files or on `PATH`. The bundled copy beating the system one is
deliberate — a release should behave the same on every machine — and the
environment variable exists so a user can still override it.

### How this was wrong for the whole of 0.1.0

The decision to bundle 7-Zip was taken, the binaries were downloaded into
`tools/sevenzip/`, and the changes to `tauri.conf.json` and `find_7z` were written
and **never applied**. The released installer therefore contained no 7-Zip at all,
`find_7z` did not look in the bundle, and there was no licence attribution.

Nothing revealed it, because the machine it was built on has 7-Zip installed, so
the system fallback always answered. It would have failed only on someone else's
computer, and only the first time they tried to install a mod.

Two tests now guard it — `the_bundle_still_ships_seven_zip` and
`the_seven_zip_licence_is_in_the_repository` in `engine::archive` — but the general
lesson is the one worth keeping: **a packaging claim is only true once the artefact
has been opened.**

## A fresh checkout

Three of the shipped binaries are not in git, because they are not ours to version:

| file | where to get it |
|---|---|
| `tools/sevenzip/7z.exe`, `7z.dll` | a 7-Zip 25.01 x64 install, or 7-zip.org |
| `tools/MBINCompiler.exe` | its GitHub release matching the game version |
| `tools/hgpaktool.exe` | built by `tools/build_hgpaktool.py` |

`tools/sevenzip/License.txt` **is** tracked, deliberately: we redistribute LGPL
software and its terms have to travel with it, so a release built on a fresh
checkout must not be missing them. See `tools/THIRD-PARTY.md`.

Without the binaries the build still succeeds and silently produces an installer
missing those tools, which is exactly how the 0.1.0 mistake happened. Check the
extracted payload, not the build log.

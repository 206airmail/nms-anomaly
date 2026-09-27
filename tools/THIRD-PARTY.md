# Third-party code bundled with nmscheck

nmscheck ships two binaries it did not write, and one it builds itself from a
vendored library. The two binaries are run as **separate processes** — nothing
here links against them — and both can be replaced by the user with their own
copy (see "Substituting your own build" below).

## MBINCompiler

- Source: <https://github.com/monkeyman192/MBINCompiler>
- Author: monkeyman192 and contributors
- Licence: **GNU Lesser General Public License, version 3 (LGPL-3.0)**
- Shipped as: `tools/MBINCompiler.exe`, unmodified, exactly as published by
  upstream.

Used to convert a compiled `.MBIN` asset back into readable XML, which is what
makes a scene comparable against the game's own copy.

The full LGPL-3.0 text, and the GPL-3.0 text it incorporates by reference, are
available from <https://www.gnu.org/licenses/lgpl-3.0.html> and
<https://www.gnu.org/licenses/gpl-3.0.html>. Upstream's copy is at
<https://github.com/monkeyman192/MBINCompiler/blob/master/LICENSE.md>.

## hgpaktool

- Source: <https://github.com/monkeyman192/HGPAKtool>
- Author: monkeyman192
- Licence: **MIT**
- Shipped as: `tools/hgpaktool.exe`, **rebuilt** — see below.

Used to extract the unmodified copy of an asset from the game's `PCBANKS`
archives.

### Why hgpaktool is rebuilt rather than copied

`pip install hgpaktool` produces a launcher in the virtualenv's `Scripts/` with
an absolute path to *that machine's* `python.exe` written into it. Shipping it
would produce a binary that runs on one computer. `tools/build_hgpaktool.py`
freezes the same package with PyInstaller into a self-contained executable
instead; the script refuses to install a build that still references the build
machine. No hgpaktool source is modified.

## MinHook, inside the session recorder

- Source: <https://github.com/TsudaKageyu/minhook>
- Author: Tsuda Kageyu and contributors
- Licence: **BSD 2-Clause**
- Shipped as: part of `tools/nmslogger/xinput9_1_0.dll`, statically linked.
  Vendored source in `hook/third_party/minhook/`, unmodified, licence text at
  `hook/third_party/minhook/LICENSE.txt`.

`tools/nmslogger/xinput9_1_0.dll` is **our own** code (`hook/`), built with
MinHook inside it. It is the recorder described in `docs/session-recording.md`:
the user installs it beside `NMS.exe` themselves, from the Sessions tab, and can
remove it there. MinHook is what lets it observe the game's file opens; the rest
of the DLL is in this repository.

## Substituting your own build

Neither binary is required, and neither is hidden. nmscheck looks for them in
this order, and the first hit wins:

1. `NMS_MBINCOMPILER` / `NMS_HGPAKTOOL` / `NMSCHECK_HOOK_DLL` — set any of them
   to the full path of your
   own build and it is used exactly as given. A path that does not exist is an
   error, never a silent fall back to the bundled copy.
2. the `tools/` folder beside the application
3. anything on `PATH`

If a tool is missing, the scene check does not run, and the report says so
rather than reporting an unchecked library as a clean one.

## 7-Zip

- Source: <https://www.7-zip.org/>
- Author: Igor Pavlov
- Licence: **GNU Lesser General Public License, version 2.1 or later**, with the
  additional unRAR restriction described below.
- Shipped as: `tools/sevenzip/7z.exe` and `tools/sevenzip/7z.dll`, unmodified,
  version 25.01, together with upstream's `License.txt`.

Used to extract downloaded mod archives. Nexus mods for this game arrive as
`.zip`, `.7z` and `.rar`, and 7-Zip is the only readily redistributable tool
that reads all three.

`7z.exe` on its own handles little beyond the 7z format; `7z.dll` is what adds
the rest, so the two ship together and the engine looks for them together.

### The unRAR restriction

7-Zip's RAR support derives from unRAR source, whose licence forbids using it
to develop a program that *creates* RAR archives. We only ever read them, and
nothing here is a RAR compressor, so the restriction is satisfied. It is
recorded because redistributing the binary carries the notice with it.

The full LGPL-2.1 text is at <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html>,
and upstream's own terms travel with the binary as `tools/sevenzip/License.txt`.

### Substituting your own build

Set `NMSCHECK_7Z` to the full path of a `7z.exe` you trust and it is used in
preference to the bundled one. Failing that, an installation under
`C:\Program Files\7-Zip` or a `7z` on `PATH` is used if the bundled copy is
absent.

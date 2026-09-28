# Third-party code bundled with Anomaly

Anomaly ships three binaries it did not write -- MBINCompiler, hgpaktool and
7-Zip -- each unmodified and exactly as its author published it, and one of its
own built around a vendored library.

Every one of the three is run as a **separate process**. Nothing here links
against any of them, and each can be replaced by the user with a copy they
trust (see "Substituting your own build" below).

None of the three is in git: only their licence texts are. Each section says
where to get the binary, and the MBINCompiler one is worth reading before you
take the newest release of it.

## MBINCompiler

- Source: <https://github.com/monkeyman192/MBINCompiler>
- Releases: <https://github.com/monkeyman192/MBINCompiler/releases>
- Author: monkeyman192 and contributors
- Licence: **GNU Lesser General Public License, version 3 (LGPL-3.0)**
- Shipped as: `tools/MBINCompiler.exe`, unmodified, exactly as published by
  upstream. Currently **v7.03.2-pre2**; ask the binary with
  `MBINCompiler.exe version`.

Used to convert a compiled `.MBIN` asset back into readable XML, which is what
makes a scene comparable against the game's own copy.

### The version has to match the game, not be the newest

This is the one place where "just take the latest release" is wrong.

MBINCompiler's output tracks the **game's build**. Handed a 7.04 decompiler and
a 7.03 game, it does not fail loudly -- it decompiles the same asset into
different XML, and every comparison this program makes against the game's own
files quietly becomes wrong. Being a version behind is much cheaper than being
confidently incorrect, which is the failure this program exists to prevent.

It is also a trap dressed as safe. At the time of writing every MBINCompiler
release *except* the one shipped here is flagged a prerelease, so GitHub's
"latest release" endpoint returns exactly what is bundled -- and would start
returning a mismatched build the moment upstream marks a newer one stable. It
would look correct right up until it silently was not.

So: take the release whose version matches the No Man's Sky you are running.
Anomaly reports the MBIN version the *mods* were built against, which is the
closest thing it can measure, and lists mods built for another one.

### Obtaining it for a fresh checkout

The binary is **not** in git. Take `MBINCompiler.exe` from the matching release
above and put it in `tools/`. Nothing else from that release is needed -- the
`libMBIN` DLLs are for programs that link against it, and nothing here does.

## hgpaktool

- Source: <https://github.com/monkeyman192/HGPAKtool>
- Releases: <https://github.com/monkeyman192/HGPAKtool/releases>
- Author: monkeyman192
- Licence: **MIT**
- Shipped as: `tools/hgpaktool.exe`, unmodified, exactly as published by
  upstream. Currently **1.1.3**; the version is in the first line of
  `hgpaktool.exe --help`.

Used to extract the unmodified copy of an asset from the game's `PCBANKS`
archives.

Unlike MBINCompiler this reads the pak *container*, which is stable across game
updates, so the newest release is the right one to take.

### Obtaining it for a fresh checkout

The binary is **not** in git. Download `hgpaktool-x86_64-pc-windows.zip` from
the releases above, and put the `hgpaktool.exe` inside it in `tools/`.

This used to say something else. `pip install hgpaktool` produces a launcher in
the virtualenv's `Scripts/` with an absolute path to *that machine's*
`python.exe` written into it, so shipping it would have produced a binary that
ran on one computer; a local script froze the package with PyInstaller instead.
Upstream now publishes a self-contained executable of its own, which makes all
of that unnecessary -- the two were measured against each other before the
change: same version, byte-identical extraction from the same 97 `.pak` files,
and upstream's is half a megabyte smaller.

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

No tool here is required, and none is hidden. Anomaly looks for them in this
order, and the first hit wins:

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
  **version 25.01 (2025-08-03)**, together with upstream's own `License.txt`.

Used to extract downloaded mod archives. Nexus mods for this game arrive as
`.zip`, `.7z` and `.rar`, and 7-Zip is the only readily redistributable tool that
reads all three.

`7z.exe` on its own handles little beyond the 7z format; `7z.dll` is what adds the
rest, so the two ship together and `engine::archive::find_7z` looks for them
together.

### The unRAR restriction

7-Zip's RAR support derives from unRAR source, whose licence forbids using it to
develop a program that *creates* RAR archives. We only ever read them and nothing
here is a RAR compressor, so the restriction is satisfied. It is recorded because
redistributing the binary carries the notice with it.

The full LGPL-2.1 text is at
<https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html>, and upstream's own
terms travel with the binary as `tools/sevenzip/License.txt`.

### Obtaining the binaries for a fresh checkout

The two binaries are **not** in git -- only the licence text is. Without them the
release bundle is built without 7-Zip and a machine that has no 7-Zip installed
cannot extract a downloaded mod. To restore them, take `7z.exe` and `7z.dll` from
a 7-Zip 25.01 x64 installation (or the official download) and put both in
`tools/sevenzip/`.

### Substituting your own build

Set `NMSCHECK_7Z` to the full path of a `7z.exe` you trust and it wins over
everything else. Failing that the bundled copy is used, and failing that an
installation under `C:\Program Files\7-Zip` or a `7z` on `PATH`.

# Builds bin\anomaly_recorder.dll -- Anomaly's session recorder, as an Atlas plugin.
#
# It includes atlas.h (vendored into src\, so this builds with no path back to
# the Atlas checkout) and links against nothing of Atlas's. It DOES bring
# MinHook, because Atlas patches no code and offers no hooking capability --
# a plugin that wants to detour CreateFileW supplies its own detour library.
#
# MinHook is referenced from hook\third_party rather than copied: it is the
# same repo and the same vendored copy the old combined DLL used, so there is
# one version of it to keep current.
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$mh = Resolve-Path (Join-Path $here '..\..\hook\third_party\minhook')

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio with the C++ x64 tools was not found.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'

$obj = Join-Path $here 'obj'
$bin = Join-Path $here 'bin'
New-Item -ItemType Directory -Force $obj, $bin | Out-Null

$sources = @(
    "$here\src\plugin.cpp", "$here\src\log.cpp", "$here\src\hooks.cpp",
    "$here\src\crash.cpp", "$here\src\util.cpp",
    "$mh\src\buffer.c", "$mh\src\hook.c", "$mh\src\trampoline.c", "$mh\src\hde\hde64.c"
) | ForEach-Object { "`"$_`"" }

$cl = @(
    'cl /nologo /LD /O2 /MT /EHsc /std:c++17 /W3 /GS /Zi /utf-8',
    '/DUNICODE /D_UNICODE',
    "/I`"$mh\include`"", "/I`"$here\src`"",
    # The trailing backslash is doubled deliberately: /Fo"...\obj\" would let
    # the C runtime read \" as an escaped quote and swallow the next argument.
    "/Fo`"$obj\\`"", "/Fd`"$obj\\`"",
    ($sources -join ' '),
    "/link /OUT:`"$bin\anomaly_recorder.dll`" /PDB:`"$bin\anomaly_recorder.pdb`" /IMPLIB:`"$obj\anomaly_recorder.lib`" /DEBUG /OPT:REF /OPT:ICF",
    'kernel32.lib user32.lib dbghelp.lib'
) -join ' '

$env:PATH = "$(Split-Path $vswhere);$env:PATH"   # vcvars64 calls vswhere itself
$ErrorActionPreference = 'Continue'              # cl writes source names to stderr
cmd /c "call `"$vcvars`" >nul 2>&1 && $cl 2>&1"
if ($LASTEXITCODE -ne 0) { throw "Recorder build failed ($LASTEXITCODE)" }
Write-Host "Built $bin\anomaly_recorder.dll"
Write-Host "Install: copy it into <game>\Binaries\Atlas\plugins\"

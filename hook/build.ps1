# Builds hook\bin\xinput9_1_0.dll with MSVC (x64, static CRT).
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio with the C++ x64 tools was not found.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'

$obj = Join-Path $here 'obj'
$bin = Join-Path $here 'bin'
New-Item -ItemType Directory -Force $obj, $bin | Out-Null

$mh = Join-Path $here 'third_party\minhook'
$sources = @(
    "$here\src\dllmain.cpp", "$here\src\log.cpp", "$here\src\hooks.cpp",
    "$here\src\crash.cpp", "$here\src\proxy.cpp", "$here\src\util.cpp",
    "$here\src\metaprobe.cpp", "$here\src\nopause.cpp",
    "$mh\src\buffer.c", "$mh\src\hook.c", "$mh\src\trampoline.c", "$mh\src\hde\hde64.c"
) | ForEach-Object { "`"$_`"" }

$cl = @(
    'cl /nologo /LD /O2 /MT /EHsc /std:c++17 /W3 /GS /Zi /utf-8',
    '/DUNICODE /D_UNICODE',
    "/I`"$mh\include`"",
    "/Fo`"$obj\\`"", "/Fd`"$obj\\`"",
    ($sources -join ' '),
    "/link /DEF:`"$here\src\exports.def`" /OUT:`"$bin\xinput9_1_0.dll`" /PDB:`"$bin\xinput9_1_0.pdb`" /IMPLIB:`"$obj\xinput9_1_0.lib`" /DEBUG /OPT:REF /OPT:ICF",
    'kernel32.lib user32.lib'
) -join ' '

$env:PATH = "$(Split-Path $vswhere);$env:PATH"   # vcvars64 calls vswhere itself
$ErrorActionPreference = 'Continue'              # cl writes source names to stderr
cmd /c "call `"$vcvars`" >nul 2>&1 && $cl 2>&1"
if ($LASTEXITCODE -ne 0) { throw "Hook build failed ($LASTEXITCODE)" }
Write-Host "Built $bin\xinput9_1_0.dll"

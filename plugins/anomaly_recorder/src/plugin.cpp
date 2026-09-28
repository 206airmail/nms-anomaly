// Anomaly session recorder -- an Atlas plugin.
//
// This file replaces what used to be dllmain.cpp in the standalone hook. The
// difference is where the entry point comes from: Windows used to load us
// because of a filename, and now Atlas loads us because we export
// AtlasPluginStart. Everything below that boundary is unchanged.
//
// ORDERING, WHICH IS THE ONE THING THAT ACTUALLY CHANGED
// ------------------------------------------------------
// As a proxy DLL we ran in DllMain, before the game had finished starting, so
// hooks were installed before the game could open a single file. As a plugin
// we run later -- Atlas loads us on a worker thread once the game is up. Some
// early file opens therefore happen before our hooks exist and are not
// recorded.
//
// That is a real, permanent loss of a small amount of very-early data, and it
// is worth stating rather than discovering later from a log that seems short.
// What Anomaly uses the recorder for -- which mod files loaded, which saves
// were written, crashes -- all happens long after load, so nothing that
// matters is affected. Module loads are caught up explicitly on install, so
// the module list stays complete.
#include "common.h"
#include "atlas.h"

// Required from Atlas API version 6.
//
// This plugin used to be pinned deliberately to the version 1 header, as a
// canary: it was the thing that caught a function pointer being inserted into
// the MIDDLE of AtlasApi, by failing in-game. That job is now done properly by
// the host, which asks every plugin which header it was built against and
// refuses one it cannot call safely -- so the canary is redundant, and being
// refused on every launch would be worse than useless.
ATLAS_DECLARE_PLUGIN_API_VERSION

thread_local bool t_inHook = false;
Config g_config;
std::wstring g_gameBinDir;
std::wstring g_gameRoot;
std::wstring g_outDir;

namespace {

const AtlasApi* g_api = nullptr;
bool g_started = false;

void LoadConfig() {
    // The same file and the same [hook] section the standalone hook used, so
    // an existing install keeps its settings and the Anomaly app needs no
    // change. Keys this plugin no longer owns (the probes, NoPause) are simply
    // not read here; they belong to other components now.
    std::wstring ini = g_outDir + L"config.ini";
    if (GetFileAttributesW(ini.c_str()) == INVALID_FILE_ATTRIBUTES) {
        WritePrivateProfileStringW(L"hook", L"LogModFileOpens", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogFileFailures", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogFirstChanceExceptions", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogModuleLoads", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogSaveWrites", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogMemory", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"DebugOutputPerSecond", L"200", ini.c_str());
    }
    g_config.logAllFileOpens = GetPrivateProfileIntW(L"hook", L"LogAllFileOpens", 0, ini.c_str()) != 0;
    g_config.logModFileOpens = GetPrivateProfileIntW(L"hook", L"LogModFileOpens", 1, ini.c_str()) != 0;
    g_config.logFileFailures = GetPrivateProfileIntW(L"hook", L"LogFileFailures", 1, ini.c_str()) != 0;
    g_config.logFirstChanceExceptions = GetPrivateProfileIntW(L"hook", L"LogFirstChanceExceptions", 1, ini.c_str()) != 0;
    g_config.logModuleLoads = GetPrivateProfileIntW(L"hook", L"LogModuleLoads", 1, ini.c_str()) != 0;
    g_config.logSaveWrites = GetPrivateProfileIntW(L"hook", L"LogSaveWrites", 1, ini.c_str()) != 0;
    g_config.logMemory = GetPrivateProfileIntW(L"hook", L"LogMemory", 1, ini.c_str()) != 0;
    g_config.debugOutputPerSecond = (int)GetPrivateProfileIntW(L"hook", L"DebugOutputPerSecond", 200, ini.c_str());
}

// Atlas already knows where the game is; asking it rather than re-deriving the
// paths is the plugin using its host's services for what they are for.
bool ResolvePaths() {
    if (!g_api || !g_api->GetGameRoot) return false;
    const wchar_t* root = g_api->GetGameRoot();
    if (!root || !*root) return false;
    g_gameRoot = root;
    g_gameBinDir = g_gameRoot + L"Binaries\\";
    g_outDir = g_gameBinDir + L"NMSLogger\\";
    CreateDirectoryW(g_outDir.c_str(), nullptr);
    return true;
}

DWORD WINAPI LateThread(LPVOID) {
    t_inHook = true;
    hooks::InstallLate();
    return 0;
}

} // namespace

extern "C" {

// Lets the Anomaly host recognise this file as its own recorder, and say which
// version it is looking at.
//
// This has to be an EXPORT, not just a constant. As a proxy DLL the marker
// reached the binary through the version export in proxy.cpp; a plugin has no
// proxy.cpp, so with nothing referencing the macro the compiler dropped the
// string entirely and the host could not identify its own recorder. A
// dllexport cannot be optimised away.
__declspec(dllexport) const char* AnomalyRecorderVersion() {
    return NMSLOG_MARKER " " RECORDER_VERSION;
}

__declspec(dllexport) int32_t AtlasPluginStart(const AtlasApi* api) {
    if (!api) return 1;
    g_api = api;
    if (!ResolvePaths()) {
        api->Log(ATLAS_LOG_ERROR, "recorder",
                 "could not resolve the game root from the host -- not starting");
        return 2;
    }

    // Order matters: the logger opens its file and pipe before any hook is
    // live, so nothing a hook produces can arrive before there is somewhere to
    // put it.
    logger::Init();
    logger::Push(Level::Info, "hook",
                 "Anomaly session recorder " RECORDER_VERSION
                 " started as an Atlas plugin (pid " +
                 std::to_string(GetCurrentProcessId()) + ")");
    LoadConfig();
    // The settings actually in force, not the ones the file appears to hold.
    // GetPrivateProfileIntW silently returns every default if the file has a
    // UTF-8 BOM before [hook] or the section is misspelt -- a failure that
    // otherwise looks exactly like the feature being broken.
    logger::Pushf(Level::Info, "hook",
                  "config: modFileOpens=%d fileFailures=%d allFileOpens=%d "
                  "firstChance=%d moduleLoads=%d saveWrites=%d memory=%d "
                  "debugPerSec=%d",
                  g_config.logModFileOpens, g_config.logFileFailures,
                  g_config.logAllFileOpens, g_config.logFirstChanceExceptions,
                  g_config.logModuleLoads, g_config.logSaveWrites,
                  g_config.logMemory, g_config.debugOutputPerSecond);
    logger::Push(Level::Info, "hook",
                 "note: running as a plugin, so hooks install after the game "
                 "has started -- a few of the earliest file opens are not "
                 "recorded. Module loads are caught up on install.");

    if (!hooks::Install()) {
        logger::Push(Level::Error, "hook", "hook installation failed");
        return 3;
    }
    crash::Install();

    // InstallLate touches modules that may not be mapped yet, and must not run
    // on the thread Atlas is using to load the rest of the plugins.
    HANDLE t = CreateThread(nullptr, 0, LateThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);

    g_started = true;
    api->Log(ATLAS_LOG_INFO, "recorder",
             "session recording active; output in Binaries\\NMSLogger\\");
    return 0;
}

// Atlas reports these; they are cheap and they mark the session boundaries the
// Anomaly app draws its timeline from.
__declspec(dllexport) void AtlasPluginOnWorldReady(int32_t generation) {
    if (!g_started) return;
    logger::Pushf(Level::Info, "session", "world ready (generation %d)", generation);
}

__declspec(dllexport) void AtlasPluginOnWorldUnloaded(void) {
    if (!g_started) return;
    logger::Push(Level::Info, "session", "world unloaded");
}

__declspec(dllexport) void AtlasPluginStop(void) {
    if (!g_started) return;
    // A save the game had open and never closed never got its closing line;
    // say so before the log ends rather than leaving a dangling entry.
    hooks::ReportUnfinishedSaves();
    logger::Push(Level::Info, "hook", "plugin stopping");
    logger::FlushSync(200);
}

} // extern "C"

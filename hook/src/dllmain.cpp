#include "common.h"

thread_local bool t_inHook = false;
Config g_config;
std::wstring g_gameBinDir;
std::wstring g_gameRoot;
std::wstring g_outDir;

namespace {

bool g_active = false;

void LoadConfig() {
    std::wstring ini = g_outDir + L"config.ini";
    if (GetFileAttributesW(ini.c_str()) == INVALID_FILE_ATTRIBUTES) {
        WritePrivateProfileStringW(L"hook", L"LogModFileOpens", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogFileFailures", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogFirstChanceExceptions", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogModuleLoads", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogSaveWrites", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"LogMemory", L"1", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"DebugOutputPerSecond", L"200", ini.c_str());
        WritePrivateProfileStringW(L"hook", L"MetaProbe", L"0", ini.c_str());
    }
    g_config.logAllFileOpens = GetPrivateProfileIntW(L"hook", L"LogAllFileOpens", 0, ini.c_str()) != 0;
    g_config.logModFileOpens = GetPrivateProfileIntW(L"hook", L"LogModFileOpens", 1, ini.c_str()) != 0;
    g_config.logFileFailures = GetPrivateProfileIntW(L"hook", L"LogFileFailures", 1, ini.c_str()) != 0;
    g_config.logFirstChanceExceptions = GetPrivateProfileIntW(L"hook", L"LogFirstChanceExceptions", 1, ini.c_str()) != 0;
    g_config.logModuleLoads = GetPrivateProfileIntW(L"hook", L"LogModuleLoads", 1, ini.c_str()) != 0;
    g_config.logSaveWrites = GetPrivateProfileIntW(L"hook", L"LogSaveWrites", 1, ini.c_str()) != 0;
    g_config.logMemory = GetPrivateProfileIntW(L"hook", L"LogMemory", 1, ini.c_str()) != 0;
    g_config.debugOutputPerSecond = (int)GetPrivateProfileIntW(L"hook", L"DebugOutputPerSecond", 200, ini.c_str());
    g_config.metaProbe = GetPrivateProfileIntW(L"hook", L"MetaProbe", 0, ini.c_str()) != 0;
    g_config.metaProbeDelaySeconds = (int)GetPrivateProfileIntW(L"hook", L"MetaProbeDelaySeconds", 90, ini.c_str());
    g_config.metaProbeScanHeap = GetPrivateProfileIntW(L"hook", L"MetaProbeScanHeap", 1, ini.c_str()) != 0;
    g_config.metaProbeHeapBudgetMB = (int)GetPrivateProfileIntW(L"hook", L"MetaProbeHeapBudgetMB", 2048, ini.c_str());
    g_config.metaProbeScanCode = GetPrivateProfileIntW(L"hook", L"MetaProbeScanCode", 0, ini.c_str()) != 0;
    g_config.instProbe = GetPrivateProfileIntW(L"hook", L"InstProbe", 0, ini.c_str()) != 0;
    g_config.instProbeBudgetMB = (int)GetPrivateProfileIntW(L"hook", L"InstProbeBudgetMB", 6144, ini.c_str());
}

void Start() {
    wchar_t exe[MAX_PATH];
    DWORD n = GetModuleFileNameW(nullptr, exe, MAX_PATH);
    if (!n || !EndsWithNoCase(exe, L"\\NMS.exe")) return;   // only instrument the game itself
    g_gameBinDir.assign(exe, n);
    g_gameBinDir.resize(g_gameBinDir.find_last_of(L'\\') + 1);
    g_gameRoot = g_gameBinDir.substr(0, g_gameBinDir.find_last_of(L'\\', g_gameBinDir.size() - 2) + 1);
    g_outDir = g_gameBinDir + L"NMSLogger\\";
    CreateDirectoryW(g_outDir.c_str(), nullptr);
    LoadConfig();

    // Order matters: the logger opens its file before any hook is live.
    logger::Init();
    logger::Push(Level::Info, "hook", "NMS Logger hook " NMSLOG_HOOK_VERSION " attached to " + ToUtf8(exe) +
                                      " (pid " + std::to_string(GetCurrentProcessId()) + ")");
    // The settings we actually ended up with, not the ones the file appears to
    // hold. config.ini is read through GetPrivateProfileIntW, which silently
    // returns every default if the file has a UTF-8 BOM in front of [hook] or
    // the section name is misspelt -- a failure that otherwise looks exactly
    // like the feature being broken.
    logger::Pushf(Level::Info, "hook",
                  "config: modFileOpens=%d fileFailures=%d allFileOpens=%d firstChance=%d "
                  "moduleLoads=%d saveWrites=%d memory=%d debugPerSec=%d "
                  "metaProbe=%d metaProbeDelay=%ds metaProbeHeap=%d metaProbeBudget=%dMB "
                  "metaProbeCode=%d instProbe=%d instProbeBudget=%dMB",
                  g_config.logModFileOpens, g_config.logFileFailures, g_config.logAllFileOpens,
                  g_config.logFirstChanceExceptions, g_config.logModuleLoads,
                  g_config.logSaveWrites, g_config.logMemory, g_config.debugOutputPerSecond,
                  g_config.metaProbe, g_config.metaProbeDelaySeconds,
                  g_config.metaProbeScanHeap, g_config.metaProbeHeapBudgetMB,
                  g_config.metaProbeScanCode, g_config.instProbe,
                  g_config.instProbeBudgetMB);
    hooks::Install();
    crash::Install();
    metaprobe::Start();
    // Runs after DllMain returns and the loader lock is released.
    HANDLE t = CreateThread(nullptr, 0, [](LPVOID) -> DWORD {
        t_inHook = true;
        hooks::InstallLate();
        return 0;
    }, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
    g_active = true;
}

} // namespace

BOOL APIENTRY DllMain(HMODULE mod, DWORD reason, LPVOID reserved) {
    switch (reason) {
        case DLL_PROCESS_ATTACH:
            DisableThreadLibraryCalls(mod);
            Start();
            break;
        case DLL_PROCESS_DETACH:
            // reserved != null: process is terminating (other threads are already gone).
            if (g_active && reserved) {
                logger::Push(Level::Info, "hook", "process detach");
                logger::FlushSync(200);
            }
            break;
    }
    return TRUE;
}

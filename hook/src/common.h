// Shared declarations for the NMS Logger hook DLL.
#pragma once

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <string>
#include <cstdint>

#define NMSLOG_HOOK_VERSION "0.2.0"
// Searched for by the host installer to recognise its own DLL.
#define NMSLOG_MARKER "NMSLOGGER_HOOK_MARKER_V1"
#define NMSLOG_PIPE_NAME L"\\\\.\\pipe\\NMSLogger"

enum class Level : int { Debug = 0, Info = 1, Warn = 2, Error = 3, Fatal = 4 };

// Set on threads that must never be observed by our own hooks
// (the logger thread, and any hook currently running on a thread).
extern thread_local bool t_inHook;

struct ReentryGuard {
    bool prev;
    ReentryGuard() : prev(t_inHook) { t_inHook = true; }
    ~ReentryGuard() { t_inHook = prev; }
};

struct Config {
    bool logModFileOpens = true;       // successful opens under GAMEDATA\MODS
    bool logFileFailures = true;       // failed file opens
    bool logAllFileOpens = false;      // diagnostic: every successful open outside MODS
    bool logFirstChanceExceptions = true;
    bool logModuleLoads = true;
    bool logSaveWrites = true;         // every write to a save slot, and its result
    bool logMemory = true;             // memory and handle use, on the heartbeat
    int  debugOutputPerSecond = 200;   // rate limit for OutputDebugString capture
    // The metadata probe (metaprobe.cpp) is the one part of this DLL that looks
    // at the game's own memory rather than the OS boundary, so it is off unless
    // asked for.
    bool metaProbe = false;
    int  metaProbeDelaySeconds = 90;   // first automatic run; 0 = on demand only
    bool metaProbeScanHeap = true;     // pass 2 over private memory, not just the image
    int  metaProbeHeapBudgetMB = 2048; // cap on that pass, so a big session stays playable
    bool metaProbeScanCode = false;    // search code sections too; strings are not kept there
    // The instance probe needs a save loaded to find anything, and it searches the
    // heap rather than the image, so it gets its own budget.
    bool instProbe = false;
    int  instProbeBudgetMB = 6144;
    // Run the instance probe the moment a save finishes writing. Nothing owns a
    // cGcPlayerStateData member -- it is the save document root -- so that is the
    // one moment its 27 inline inventories are certain to be materialised.
    bool instProbeOnSave = false;
    // Settles whether the runtime holds item ids as text; see metaprobe.cpp.
    bool stringHunt = false;
};
extern Config g_config;

// Directory containing NMS.exe (with trailing backslash) and our output dir.
extern std::wstring g_gameBinDir;
extern std::wstring g_gameRoot;   // folder containing Binaries\, with trailing backslash
extern std::wstring g_outDir;

// ---- logger (log.cpp) ----
namespace logger {
    bool Init();                                  // must run before hooks are enabled
    void Push(Level lvl, const char* cat, std::string msg);
    void Pushf(Level lvl, const char* cat, const char* fmt, ...);
    // Synchronously write everything queued. Safe-ish to call from a crash handler.
    void FlushSync(DWORD timeoutMs);
    void Shutdown();
}

// ---- hooks (hooks.cpp) ----
namespace hooks {
    bool Install();
    void InstallLate();
    std::string HeartbeatReport();
    // "workingSet=... private=... handles=..." -- telemetry for the host to
    // trend, kept apart from the human-readable heartbeat line.
    std::string MemoryReport();
    // Saves the game had open and never closed, for the crash report.
    std::string InFlightSavesReport();
    // Called on the way out: a save still open never got its closing line.
    void ReportUnfinishedSaves();
    void RecordRecentFile(const wchar_t* path, bool ok);
    // Copies the most recent file opens (newest first) into out, one per line.
    std::string RecentFilesReport();
    // Original (un-hooked) functions, for our own I/O.
    extern decltype(&CreateFileW) o_CreateFileW;
    extern decltype(&WriteFile) o_WriteFile;
    extern decltype(&SetUnhandledExceptionFilter) o_SetUnhandledExceptionFilter;
}

// ---- metadata probe (metaprobe.cpp) ----
namespace metaprobe {
    // Spawns the probe thread, or does nothing if Config::metaProbe is off.
    void Start();
    // Called from the file hooks when a save has finished writing. Cheap and
    // lock-free: it sets a flag the probe thread is already polling, because the
    // caller is inside a hook on the game's own thread.
    void OnSaveWritten();
}

// ---- crash handling (crash.cpp) ----
namespace crash {
    void Install();
    LONG WINAPI UnhandledFilter(EXCEPTION_POINTERS* ep);
    extern LPTOP_LEVEL_EXCEPTION_FILTER g_gameFilter;
}

// ---- utilities (util.cpp) ----
std::string ToUtf8(const wchar_t* s, int len = -1);
std::string ToUtf8(const std::wstring& s);
// "NMS.exe+0x1234" style description of a code address.
std::string DescribeAddress(const void* addr);
bool ContainsNoCase(const wchar_t* hay, const wchar_t* needle);
bool EndsWithNoCase(const wchar_t* hay, const wchar_t* needle);
Level ClassifyText(const char* text, size_t len, Level fallback);

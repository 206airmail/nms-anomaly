// Shared declarations for the Anomaly session recorder.
//
// WHAT THIS IS
// ------------
// Everything Anomaly wants from inside the running game -- which files the
// game opened, which saves it wrote, how much memory it used, and why it
// crashed -- now lives in an Atlas plugin instead of in its own proxy DLL.
//
// That is not tidiness. Only one mod can be named xinput9_1_0.dll, which is
// how a DLL gets loaded into NMS at all, so Atlas and a separate Anomaly hook
// could never be installed at the same time. Making Anomaly a plugin is what
// lets both exist. It also means Anomaly consumes exactly the public contract
// a third party does, and cannot quietly acquire privileges plugin authors
// lack.
//
// WHAT IT KEEPS FROM THE OLD HOOK, DELIBERATELY
// ---------------------------------------------
// The output paths and the config file are unchanged: it still reads
// <game>\Binaries\NMSLogger\config.ini under [hook], still writes
// hook_latest.log beside it, and still speaks the same JSON protocol over the
// same named pipe. So the Anomaly desktop app's Sessions tab keeps working
// with no changes on its side.
//
// WHAT IT BRINGS ITSELF
// ---------------------
// MinHook. Atlas patches no code and offers no hooking capability, so a plugin
// that wants to detour CreateFileW brings its own detour library -- as
// documented in atlas.h. Be aware that two plugins hooking the same function
// have nobody arbitrating between them.
#pragma once

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <string>
#include <cstdint>

#define RECORDER_VERSION "0.3.0"
// Searched for by the Anomaly host to recognise its own recorder.
#define NMSLOG_MARKER "NMSLOGGER_HOOK_MARKER_V1"
#define NMSLOG_PIPE_NAME L"\\\\.\\pipe\\NMSLogger"
// The host reads this from the hello line; keep reporting the hook version it
// already understands rather than inventing a new field for it to ignore.
#define NMSLOG_HOOK_VERSION RECORDER_VERSION

enum class Level : int { Debug = 0, Info = 1, Warn = 2, Error = 3, Fatal = 4 };

// Set on threads that must never be observed by our own hooks (the logger
// thread, and any hook currently running on a thread).
extern thread_local bool t_inHook;

struct ReentryGuard {
    bool prev;
    ReentryGuard() : prev(t_inHook) { t_inHook = true; }
    ~ReentryGuard() { t_inHook = prev; }
};

struct Config {
    bool logModFileOpens = true;       // successful opens under GAMEDATA\MODS
    bool logFileFailures = true;       // failed file opens
    bool logAllFileOpens = false;      // diagnostic: every successful open
    bool logFirstChanceExceptions = true;
    bool logModuleLoads = true;
    bool logSaveWrites = true;         // every write to a save slot, and its result
    bool logMemory = true;             // memory and handle use, on the heartbeat
    int  debugOutputPerSecond = 200;   // rate limit for OutputDebugString capture
};
extern Config g_config;

extern std::wstring g_gameBinDir;   // folder containing NMS.exe, trailing backslash
extern std::wstring g_gameRoot;     // folder containing Binaries\, trailing backslash
extern std::wstring g_outDir;       // <game>\Binaries\NMSLogger\

// ---- logger (log.cpp) ----
namespace logger {
bool Init();
void Push(Level lvl, const char* cat, std::string msg);
void Pushf(Level lvl, const char* cat, const char* fmt, ...);
void FlushSync(DWORD timeoutMs);
void Shutdown();
}

// ---- hooks (hooks.cpp) ----
namespace hooks {
bool Install();
void InstallLate();
std::string HeartbeatReport();
std::string MemoryReport();
std::string InFlightSavesReport();
void ReportUnfinishedSaves();
void RecordRecentFile(const wchar_t* path, bool ok);
std::string RecentFilesReport();
extern decltype(&CreateFileW) o_CreateFileW;
extern decltype(&WriteFile) o_WriteFile;
extern decltype(&SetUnhandledExceptionFilter) o_SetUnhandledExceptionFilter;
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
std::string DescribeAddress(const void* addr);
bool ContainsNoCase(const wchar_t* hay, const wchar_t* needle);
bool EndsWithNoCase(const wchar_t* hay, const wchar_t* needle);
Level ClassifyText(const char* text, size_t len, Level fallback);

// Win32 API hooks. These are all stable OS entry points, so they keep working
// across game patches (no signatures / offsets into NMS.exe).
#include "common.h"
#include "MinHook.h"
#define PSAPI_VERSION 2   // K32EnumProcessModules from kernel32, no psapi.lib
#include <psapi.h>
#include <winternl.h>
#include <intrin.h>
#include <cwctype>
#include <iterator>
#include <unordered_set>
#include <vector>

#pragma intrinsic(_ReturnAddress)

namespace hooks {
decltype(&CreateFileW) o_CreateFileW = &CreateFileW;   // not hooked; our I/O is guarded by t_inHook
decltype(&WriteFile) o_WriteFile = nullptr;
decltype(&SetUnhandledExceptionFilter) o_SetUnhandledExceptionFilter = nullptr;
}
using namespace hooks;

namespace {

typedef NTSTATUS (NTAPI *NtCreateFile_t)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES, PIO_STATUS_BLOCK, PLARGE_INTEGER,
                                         ULONG, ULONG, ULONG, ULONG, PVOID, ULONG);
typedef NTSTATUS (NTAPI *NtOpenFile_t)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES, PIO_STATUS_BLOCK, ULONG, ULONG);
typedef ULONG (NTAPI *RtlNtStatusToDosError_t)(NTSTATUS);
NtCreateFile_t o_NtCreateFile = nullptr;
NtOpenFile_t o_NtOpenFile = nullptr;
RtlNtStatusToDosError_t o_RtlNtStatusToDosError = nullptr;
decltype(&OutputDebugStringA) o_OutputDebugStringA = nullptr;
decltype(&OutputDebugStringW) o_OutputDebugStringW = nullptr;
decltype(&MessageBoxA) o_MessageBoxA = nullptr;
decltype(&MessageBoxW) o_MessageBoxW = nullptr;
decltype(&TerminateProcess) o_TerminateProcess = nullptr;
typedef VOID (NTAPI *RtlExitUserProcess_t)(LONG);
RtlExitUserProcess_t o_RtlExitUserProcess = nullptr;
// Writes are accounted at the NT layer for the same reason opens are: WriteFile
// calls NtWriteFile, so counting both would count every byte twice.
typedef NTSTATUS (NTAPI *NtWriteFile_t)(HANDLE, HANDLE, PVOID, PVOID, PIO_STATUS_BLOCK, PVOID, ULONG,
                                        PLARGE_INTEGER, PULONG);
typedef NTSTATUS (NTAPI *NtClose_t)(HANDLE);
NtWriteFile_t o_NtWriteFile = nullptr;
NtClose_t o_NtClose = nullptr;

constexpr NTSTATUS kStatusPending = 0x00000103L;

// ---------------------------------------------------------------- dedupe
SRWLOCK g_seenLock = SRWLOCK_INIT;
std::unordered_set<std::wstring> g_seen;

// Returns true the first time a key is seen.
bool FirstTime(const wchar_t* path, DWORD extra) {
    std::wstring key(path);
    for (auto& c : key) c = towlower(c);
    key += L'|';
    key += std::to_wstring(extra);
    AcquireSRWLockExclusive(&g_seenLock);
    bool fresh = g_seen.size() < 200000 && g_seen.insert(std::move(key)).second;
    ReleaseSRWLockExclusive(&g_seenLock);
    return fresh;
}

// ---------------------------------------------------------- recent files
struct RecentFile { wchar_t path[MAX_PATH]; bool ok; ULONGLONG tick; };
constexpr int kRecent = 48;
RecentFile g_recent[kRecent];
volatile LONG g_recentIdx = -1;
SRWLOCK g_recentLock = SRWLOCK_INIT;

// ------------------------------------------------------------- FullLog.txt
volatile HANDLE g_fullLog = nullptr;
SRWLOCK g_fullLogLock = SRWLOCK_INIT;
std::string g_fullLogPending;

void EmitGameLogLines(const char* data, DWORD len) {
    AcquireSRWLockExclusive(&g_fullLogLock);
    g_fullLogPending.append(data, len);
    size_t start = 0, nl;
    while ((nl = g_fullLogPending.find('\n', start)) != std::string::npos) {
        size_t end = nl;
        while (end > start && (g_fullLogPending[end - 1] == '\r')) --end;
        if (end > start) {
            const char* p = g_fullLogPending.data() + start;
            logger::Push(ClassifyText(p, end - start, Level::Info), "gamelog", std::string(p, end - start));
        }
        start = nl + 1;
    }
    g_fullLogPending.erase(0, start);
    if (g_fullLogPending.size() > 65536) {   // no newline for a long time; emit as-is
        logger::Push(Level::Info, "gamelog", g_fullLogPending);
        g_fullLogPending.clear();
    }
    ReleaseSRWLockExclusive(&g_fullLogLock);
}

// --------------------------------------------------------------- save files
// NMS keeps its saves in %APPDATA%\HelloGames\NMS\st_<steamid>\ as save<N>.hg
// with a manifest mf_save<N>.hg beside it. A save that fails to write, or that
// is cut short by a full disk or a crash, is the worst thing that can happen to
// a player and the game says nothing about it at all -- so every write to one of
// those files is accounted for, and the total is reported when the game closes
// the file, which is the moment the save is actually finished.
//
// Accounting happens at NtWriteFile, not at WriteFile, because WriteFile calls
// it: counting both would double every byte.
struct SaveFile {
    HANDLE handle;
    wchar_t name[96];
    LONG64 bytes;
    LONG writes;
    LONG failures;
    DWORD lastError;
    ULONGLONG firstTick;
};

constexpr int kSaveSlots = 8;   // the game has never had more than two open at once
SaveFile g_saves[kSaveSlots];
SRWLOCK g_saveLock = SRWLOCK_INIT;
// Read on every write and every close, so it is checked before taking the lock.
volatile LONG g_savesOpen = 0;
volatile LONG64 g_saveBytes = 0;
volatile LONG g_saveWrites = 0;
volatile LONG g_saveFailures = 0;

bool IsSaveFile(const wchar_t* path) {
    return ContainsNoCase(path, L"\\HelloGames\\NMS\\") && EndsWithNoCase(path, L".hg");
}

const wchar_t* FileNameOf(const wchar_t* path) {
    const wchar_t* slash = wcsrchr(path, L'\\');
    return slash ? slash + 1 : path;
}

// Report one finished save, and stop tracking it. Called with the lock held.
void FinishSave(SaveFile& save) {
    if (save.writes > 0) {
        double mb = (double)save.bytes / (1024.0 * 1024.0);
        ULONGLONG ms = GetTickCount64() - save.firstTick;
        if (save.failures > 0)
            logger::Pushf(Level::Error, "save",
                          "FAILED writing %s: error %lu after %lld bytes in %ld writes over %llu ms",
                          ToUtf8(save.name).c_str(), save.lastError, (long long)save.bytes,
                          save.writes, ms);
        else
            logger::Pushf(Level::Info, "save", "wrote %s: %lld bytes in %ld writes over %llu ms (%.1f MB)",
                          ToUtf8(save.name).c_str(), (long long)save.bytes, save.writes, ms, mb);
    }
    save.handle = nullptr;
    save.bytes = 0;
    save.writes = 0;
    save.failures = 0;
    save.lastError = 0;
    InterlockedDecrement(&g_savesOpen);
}

void TrackSave(HANDLE h, const wchar_t* path) {
    AcquireSRWLockExclusive(&g_saveLock);
    int free = -1;
    for (int i = 0; i < kSaveSlots; ++i) {
        // A handle value the kernel has recycled for a different save: whatever
        // was counted against it is finished, one way or another.
        if (g_saves[i].handle == h) FinishSave(g_saves[i]);
        if (g_saves[i].handle == nullptr && free < 0) free = i;
    }
    if (free >= 0) {
        g_saves[free].handle = h;
        wcsncpy_s(g_saves[free].name, FileNameOf(path), _TRUNCATE);
        g_saves[free].firstTick = GetTickCount64();
        InterlockedIncrement(&g_savesOpen);
    }
    ReleaseSRWLockExclusive(&g_saveLock);
}

void NoteSaveWrite(HANDLE h, ULONG bytes, bool ok, DWORD err) {
    AcquireSRWLockExclusive(&g_saveLock);
    for (int i = 0; i < kSaveSlots; ++i) {
        if (g_saves[i].handle != h) continue;
        if (g_saves[i].writes == 0) g_saves[i].firstTick = GetTickCount64();
        ++g_saves[i].writes;
        InterlockedIncrement(&g_saveWrites);
        if (ok) {
            g_saves[i].bytes += bytes;
            InterlockedAdd64(&g_saveBytes, (LONG64)bytes);
        } else {
            ++g_saves[i].failures;
            g_saves[i].lastError = err;
            InterlockedIncrement(&g_saveFailures);
        }
        break;
    }
    ReleaseSRWLockExclusive(&g_saveLock);
}

void CloseSave(HANDLE h) {
    AcquireSRWLockExclusive(&g_saveLock);
    for (int i = 0; i < kSaveSlots; ++i)
        if (g_saves[i].handle == h) FinishSave(g_saves[i]);
    ReleaseSRWLockExclusive(&g_saveLock);
}

// ------------------------------------------------------ debug output limit
volatile LONG64 g_dbgWindow = 0;
volatile LONG g_dbgCount = 0;
volatile LONG g_dbgDropped = 0;

bool DebugOutputAllowed() {
    LONG64 win = (LONG64)(GetTickCount64() / 1000);
    LONG64 cur = g_dbgWindow;
    if (win != cur && InterlockedCompareExchange64(&g_dbgWindow, win, cur) == cur) {
        InterlockedExchange(&g_dbgCount, 0);
        LONG dropped = InterlockedExchange(&g_dbgDropped, 0);
        if (dropped)
            logger::Pushf(Level::Warn, "debugout", "%ld debug messages suppressed (rate limit %d/s)",
                          dropped, g_config.debugOutputPerSecond);
    }
    if (InterlockedIncrement(&g_dbgCount) > g_config.debugOutputPerSecond) {
        InterlockedIncrement(&g_dbgDropped);
        return false;
    }
    return true;
}

std::string CallerModule(void* ret) {
    std::string d = DescribeAddress(ret);
    size_t plus = d.find('+');
    return plus == std::string::npos ? d : d.substr(0, plus);
}

void OnDebugString(std::string s, void* ret) {
    while (!s.empty() && (s.back() == '\n' || s.back() == '\r' || s.back() == ' ')) s.pop_back();
    if (s.empty() || !DebugOutputAllowed()) return;
    Level lvl = ClassifyText(s.data(), s.size(), Level::Debug);
    logger::Push(lvl, "debugout", "[" + CallerModule(ret) + "] " + s);
}

// --------------------------------------------------------------- file opens
volatile LONG64 g_gameDataOpens = 0;

void OnFileOpen(const wchar_t* name, HANDLE h, DWORD err) {
    if (!name || !name[0]) return;
    if (name[0] == L'\\' && name[1] == L'\\' && (name[2] == L'.' || name[2] == L'?') && name[3] == L'\\' &&
        !ContainsNoCase(name, L":\\"))
        return;   // devices, pipes
    bool ok = h != INVALID_HANDLE_VALUE;
    RecordRecentFile(name, ok);

    bool inMods = ContainsNoCase(name, L"\\GAMEDATA\\MODS\\");
    if (ContainsNoCase(name, L"\\GAMEDATA\\")) InterlockedIncrement64(&g_gameDataOpens);
    if (ok) {
        if (EndsWithNoCase(name, L"\\FullLog.txt")) {
            g_fullLog = h;
            logger::Push(Level::Debug, "file", "game opened its log: " + ToUtf8(name));
        } else if (h == g_fullLog) {
            g_fullLog = nullptr;   // handle value was recycled for another file
        }
        // Tracked whether or not it was opened for writing: a read-only handle
        // simply never records a write, and the access mask is not passed down
        // to here. Nothing is reported for a save that was only read.
        if (g_config.logSaveWrites && IsSaveFile(name)) TrackSave(h, name);
        if (inMods && g_config.logModFileOpens && FirstTime(name, 0)) {
            // Directories opened without FILE_DIRECTORY_FILE (e.g. backup semantics) aren't mod files.
            DWORD attrs = GetFileAttributesW(name);
            if (attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY)) return;
            logger::Push(Level::Info, "modfile", "loaded " + ToUtf8(name));
        }
        else if (!inMods && g_config.logAllFileOpens && FirstTime(name, 0))
            logger::Push(Level::Debug, "file", "opened " + ToUtf8(name));
        return;
    }
    if (!g_config.logFileFailures || !FirstTime(name, err)) return;
    // Only failures inside the game install matter; probes elsewhere (save slots,
    // driver caches, Windows crypto cache) are routine and kept at debug level.
    bool notFound = err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND;
    bool inGame = _wcsnicmp(name, g_gameRoot.c_str(), g_gameRoot.size()) == 0;
    Level lvl = inMods ? Level::Warn : !inGame ? Level::Debug : notFound ? Level::Info : Level::Warn;
    char msg[64];
    snprintf(msg, sizeof(msg), "open failed (error %lu): ", err);
    logger::Push(lvl, inMods ? "modfile" : "file", msg + ToUtf8(name));
}

// ------------------------------------------------------------------- hooks
// File opens are hooked at the ntdll layer: every Win32/CRT open (CreateFileW/A,
// CreateFile2, fopen, ...) ends up here, and the path has already been made
// absolute with backslashes. NMS uses forward-slash relative paths, which a
// CreateFileW-level hook would have to normalise itself.
constexpr ULONG kFileDirectoryFile = 0x1;   // FILE_DIRECTORY_FILE

std::wstring PathOfHandle(HANDLE h) {
    wchar_t buf[1024];
    DWORD n = GetFinalPathNameByHandleW(h, buf, (DWORD)std::size(buf), FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
    if (!n || n >= std::size(buf)) return {};
    std::wstring p(buf, n);
    if (p.rfind(L"\\\\?\\UNC\\", 0) == 0) return L"\\\\" + p.substr(8);
    if (p.rfind(L"\\\\?\\", 0) == 0) p.erase(0, 4);
    return p;
}

void OnNtOpen(POBJECT_ATTRIBUTES oa, NTSTATUS st, PHANDLE ph, ULONG options) {
    if (!oa || !oa->ObjectName || !oa->ObjectName->Buffer) return;
    bool isDir = (options & kFileDirectoryFile) != 0;
    if (isDir && !g_config.logAllFileOpens) return;   // directory enumeration
    const UNICODE_STRING* us = oa->ObjectName;
    std::wstring path(us->Buffer, us->Length / sizeof(wchar_t));
    if (oa->RootDirectory) {
        // Opened relative to an already-open directory handle.
        std::wstring root = PathOfHandle(oa->RootDirectory);
        if (root.empty()) return;
        path = root + (path.empty() || path[0] == L'\\' ? L"" : L"\\") + path;
    }
    if (path.rfind(L"\\??\\UNC\\", 0) == 0) path = L"\\\\" + path.substr(8);
    else if (path.rfind(L"\\??\\", 0) == 0) path.erase(0, 4);
    if (isDir) {
        if (st >= 0 && ContainsNoCase(path.c_str(), L"\\GAMEDATA") && FirstTime(path.c_str(), 1))
            logger::Push(Level::Debug, "file", "opened directory " + ToUtf8(path));
        return;
    }
    if (path.size() < 3 || path[1] != L':' || path[2] != L'\\') {
        if (path.rfind(L"\\\\", 0) != 0) return;   // devices, pipes, \Device\...
    }
    bool ok = st >= 0;
    DWORD err = ok ? 0 : (o_RtlNtStatusToDosError ? o_RtlNtStatusToDosError(st) : (DWORD)st);
    OnFileOpen(path.c_str(), ok && ph ? *ph : INVALID_HANDLE_VALUE, err);
}

volatile LONG64 g_openCount = 0;   // for the heartbeat: proves the hook is still live

NTSTATUS NTAPI hk_NtCreateFile(PHANDLE h, ACCESS_MASK access, POBJECT_ATTRIBUTES oa, PIO_STATUS_BLOCK iosb,
                               PLARGE_INTEGER alloc, ULONG attrs, ULONG share, ULONG disp, ULONG options,
                               PVOID ea, ULONG eaLen) {
    NTSTATUS st = o_NtCreateFile(h, access, oa, iosb, alloc, attrs, share, disp, options, ea, eaLen);
    InterlockedIncrement64(&g_openCount);
    if (!t_inHook) {
        DWORD le = GetLastError();
        ReentryGuard g;
        OnNtOpen(oa, st, h, options);
        SetLastError(le);
    }
    return st;
}

NTSTATUS NTAPI hk_NtOpenFile(PHANDLE h, ACCESS_MASK access, POBJECT_ATTRIBUTES oa, PIO_STATUS_BLOCK iosb,
                             ULONG share, ULONG options) {
    NTSTATUS st = o_NtOpenFile(h, access, oa, iosb, share, options);
    InterlockedIncrement64(&g_openCount);
    if (!t_inHook) {
        DWORD le = GetLastError();
        ReentryGuard g;
        OnNtOpen(oa, st, h, options);
        SetLastError(le);
    }
    return st;
}

NTSTATUS NTAPI hk_NtWriteFile(HANDLE h, HANDLE ev, PVOID apc, PVOID apcCtx, PIO_STATUS_BLOCK iosb,
                              PVOID buf, ULONG len, PLARGE_INTEGER off, PULONG key) {
    NTSTATUS st = o_NtWriteFile(h, ev, apc, apcCtx, iosb, buf, len, off, key);
    // The common case is a write to something that is not a save, and then this
    // costs one read of a counter.
    if (g_savesOpen && h && !t_inHook) {
        DWORD le = GetLastError();
        ReentryGuard g;
        // How much actually landed, which for a disk going full is the number
        // that matters. Not available yet for an asynchronous write.
        ULONG wrote = (st >= 0 && st != kStatusPending && iosb) ? (ULONG)iosb->Information : len;
        NoteSaveWrite(h, wrote, st >= 0,
                      o_RtlNtStatusToDosError ? o_RtlNtStatusToDosError(st) : (DWORD)st);
        SetLastError(le);
    }
    return st;
}

NTSTATUS NTAPI hk_NtClose(HANDLE h) {
    if (h && !t_inHook) {
        if (h == g_fullLog) g_fullLog = nullptr;
        if (g_savesOpen) {
            DWORD le = GetLastError();
            ReentryGuard g;
            // Closing the file is the moment a save is finished, and the only
            // point at which "27 MB in 12 writes" can be said at all.
            CloseSave(h);
            SetLastError(le);
        }
    }
    return o_NtClose(h);
}

BOOL WINAPI hk_WriteFile(HANDLE h, LPCVOID buf, DWORD n, LPDWORD written, LPOVERLAPPED ov) {
    if (h == g_fullLog && h && buf && n && !t_inHook) {
        DWORD err = GetLastError();
        ReentryGuard g;
        EmitGameLogLines((const char*)buf, n);
        SetLastError(err);
    }
    return o_WriteFile(h, buf, n, written, ov);
}

void WINAPI hk_OutputDebugStringA(LPCSTR s) {
    if (!t_inHook && s) {
        ReentryGuard g;
        OnDebugString(s, _ReturnAddress());
    }
    ReentryGuard g;   // stop any nested A/W call from being logged twice
    o_OutputDebugStringA(s);
}

void WINAPI hk_OutputDebugStringW(LPCWSTR s) {
    if (!t_inHook && s) {
        ReentryGuard g;
        OnDebugString(ToUtf8(s), _ReturnAddress());
    }
    ReentryGuard g;
    o_OutputDebugStringW(s);
}

void LogDialog(const std::string& caption, const std::string& text) {
    logger::Push(Level::Error, "dialog", "message box \"" + caption + "\": " + text);
    logger::FlushSync(1000);
}

int WINAPI hk_MessageBoxA(HWND w, LPCSTR text, LPCSTR cap, UINT type) {
    if (t_inHook) return o_MessageBoxA(w, text, cap, type);
    ReentryGuard g;
    LogDialog(cap ? cap : "", text ? text : "");
    return o_MessageBoxA(w, text, cap, type);
}

int WINAPI hk_MessageBoxW(HWND w, LPCWSTR text, LPCWSTR cap, UINT type) {
    if (t_inHook) return o_MessageBoxW(w, text, cap, type);
    ReentryGuard g;
    LogDialog(ToUtf8(cap ? cap : L""), ToUtf8(text ? text : L""));
    return o_MessageBoxW(w, text, cap, type);
}

// Games install their own crash filter (NMS writes NMS_crash_*.dmp). We keep
// ours on top and chain to whatever the game registered.
LPTOP_LEVEL_EXCEPTION_FILTER WINAPI hk_SetUnhandledExceptionFilter(LPTOP_LEVEL_EXCEPTION_FILTER f) {
    if (f == crash::UnhandledFilter) return o_SetUnhandledExceptionFilter(f);
    LPTOP_LEVEL_EXCEPTION_FILTER prev = crash::g_gameFilter;
    crash::g_gameFilter = f;
    if (!t_inHook) {
        ReentryGuard g;
        logger::Push(Level::Debug, "crash", "game registered crash handler at " + DescribeAddress((void*)f));
    }
    o_SetUnhandledExceptionFilter(crash::UnhandledFilter);
    return prev;
}

BOOL WINAPI hk_TerminateProcess(HANDLE h, UINT code) {
    if (!t_inHook && (h == GetCurrentProcess() || GetProcessId(h) == GetCurrentProcessId())) {
        ReentryGuard g;
        logger::Pushf(Level::Warn, "exit", "TerminateProcess on self, exit code %u (0x%X), called from %s",
                      code, code, DescribeAddress(_ReturnAddress()).c_str());
        hooks::ReportUnfinishedSaves();
        logger::FlushSync(1500);
    }
    return o_TerminateProcess(h, code);
}

VOID NTAPI hk_RtlExitUserProcess(LONG code) {
    if (!t_inHook) {
        ReentryGuard g;
        logger::Pushf(code == 0 ? Level::Info : Level::Warn, "exit",
                      "process exiting, exit code %ld (0x%lX)", code, (unsigned long)code);
        hooks::ReportUnfinishedSaves();
        logger::FlushSync(1500);
    }
    o_RtlExitUserProcess(code);
}

// ----------------------------------------------------------- module loads
struct UNICODE_STR { USHORT Length, MaximumLength; PWSTR Buffer; };
struct DLL_NOTIFY_DATA { ULONG Flags; const UNICODE_STR* FullDllName; const UNICODE_STR* BaseDllName; PVOID DllBase; ULONG SizeOfImage; };
typedef VOID (CALLBACK *DllNotifyFn)(ULONG reason, const DLL_NOTIFY_DATA* data, PVOID ctx);
typedef LONG (NTAPI *LdrRegisterDllNotification_t)(ULONG, DllNotifyFn, PVOID, PVOID*);
PVOID g_dllCookie = nullptr;

VOID CALLBACK OnDllNotify(ULONG reason, const DLL_NOTIFY_DATA* d, PVOID) {
    if (!d || !d->FullDllName || t_inHook) return;
    ReentryGuard g;
    std::string path = ToUtf8(d->FullDllName->Buffer, d->FullDllName->Length / 2);
    if (reason == 1)
        logger::Pushf(Level::Info, "module", "loaded %s at 0x%p", path.c_str(), d->DllBase);
    else
        logger::Push(Level::Debug, "module", "unloaded " + path);
}

void LogLoadedModules() {
    HMODULE mods[1024];
    DWORD needed = 0;
    if (!K32EnumProcessModules(GetCurrentProcess(), mods, sizeof(mods), &needed)) return;
    std::string list;
    for (DWORD i = 0; i < needed / sizeof(HMODULE) && i < 1024; ++i) {
        wchar_t p[MAX_PATH];
        if (GetModuleFileNameW(mods[i], p, MAX_PATH)) { list += "\n    "; list += ToUtf8(p); }
    }
    logger::Push(Level::Info, "module", "modules loaded at startup:" + list);
}

template <typename T>
bool Hook(const wchar_t* mod, const char* fn, void* detour, T* orig) {
    MH_STATUS s = MH_CreateHookApi(mod, fn, detour, reinterpret_cast<void**>(orig));
    if (s != MH_OK) {
        logger::Pushf(Level::Warn, "hook", "could not hook %s!%s: %s", ToUtf8(mod).c_str(), fn, MH_StatusToString(s));
        return false;
    }
    return true;
}

} // namespace

namespace hooks {

void RecordRecentFile(const wchar_t* path, bool ok) {
    LONG i = (InterlockedIncrement(&g_recentIdx)) % kRecent;
    AcquireSRWLockExclusive(&g_recentLock);
    wcsncpy_s(g_recent[i].path, path, _TRUNCATE);
    g_recent[i].ok = ok;
    g_recent[i].tick = GetTickCount64();
    ReleaseSRWLockExclusive(&g_recentLock);
}

std::string RecentFilesReport() {
    std::string out;
    if (!TryAcquireSRWLockShared(&g_recentLock)) return "    (unavailable - lock held)\n";
    LONG last = g_recentIdx;
    ULONGLONG now = GetTickCount64();
    for (int k = 0; k < kRecent && last - k >= 0; ++k) {
        const RecentFile& r = g_recent[(last - k) % kRecent];
        if (!r.path[0]) continue;
        char head[48];
        snprintf(head, sizeof(head), "    %6.1fs ago %s ", (now - r.tick) / 1000.0, r.ok ? "ok  " : "FAIL");
        out += head;
        out += ToUtf8(r.path);
        out += "\n";
    }
    ReleaseSRWLockShared(&g_recentLock);
    return out;
}

bool Install() {
    if (MH_Initialize() != MH_OK) {
        logger::Push(Level::Error, "hook", "MinHook failed to initialise; running without API hooks");
        return false;
    }
    o_RtlNtStatusToDosError = (RtlNtStatusToDosError_t)GetProcAddress(GetModuleHandleW(L"ntdll"), "RtlNtStatusToDosError");
    Hook(L"ntdll", "NtCreateFile", &hk_NtCreateFile, &o_NtCreateFile);
    Hook(L"ntdll", "NtOpenFile", &hk_NtOpenFile, &o_NtOpenFile);
    Hook(L"kernelbase", "WriteFile", &hk_WriteFile, &o_WriteFile);
    if (g_config.logSaveWrites) {
        Hook(L"ntdll", "NtWriteFile", &hk_NtWriteFile, &o_NtWriteFile);
        Hook(L"ntdll", "NtClose", &hk_NtClose, &o_NtClose);
    }
    Hook(L"kernelbase", "OutputDebugStringA", &hk_OutputDebugStringA, &o_OutputDebugStringA);
    Hook(L"kernelbase", "OutputDebugStringW", &hk_OutputDebugStringW, &o_OutputDebugStringW);
    Hook(L"kernelbase", "SetUnhandledExceptionFilter", &hk_SetUnhandledExceptionFilter, &o_SetUnhandledExceptionFilter);
    Hook(L"kernelbase", "TerminateProcess", &hk_TerminateProcess, &o_TerminateProcess);
    Hook(L"ntdll", "RtlExitUserProcess", &hk_RtlExitUserProcess, &o_RtlExitUserProcess);
    MH_STATUS s = MH_EnableHook(MH_ALL_HOOKS);
    if (s != MH_OK)
        logger::Pushf(Level::Error, "hook", "enabling hooks failed: %s", MH_StatusToString(s));

    if (g_config.logModuleLoads) {
        LogLoadedModules();
        auto reg = (LdrRegisterDllNotification_t)GetProcAddress(GetModuleHandleW(L"ntdll"), "LdrRegisterDllNotification");
        if (reg) reg(0, OnDllNotify, nullptr, &g_dllCookie);
    }
    return s == MH_OK;
}

// Memory, handles and the session's save totals, as key=value pairs.
//
// A separate line from the heartbeat, and deliberately not prose: the host
// trends these across a session, and "the working set grew nine gigabytes over
// three hours" is the answer to the commonest complaint about a heavily modded
// install. Bytes rather than megabytes, so nothing is rounded before it is
// compared.
std::string MemoryReport() {
    PROCESS_MEMORY_COUNTERS_EX pmc{};
    pmc.cb = sizeof(pmc);
    GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS*)&pmc, sizeof(pmc));
    MEMORYSTATUSEX sys{};
    sys.dwLength = sizeof(sys);
    GlobalMemoryStatusEx(&sys);
    DWORD handles = 0;
    GetProcessHandleCount(GetCurrentProcess(), &handles);

    char b[640];
    snprintf(b, sizeof(b),
             "workingSet=%llu peakWorkingSet=%llu private=%llu peakPaged=%llu pageFaults=%lu "
             "handles=%lu systemFree=%llu systemTotal=%llu saveBytes=%lld saveWrites=%ld saveFailures=%ld",
             (unsigned long long)pmc.WorkingSetSize, (unsigned long long)pmc.PeakWorkingSetSize,
             (unsigned long long)pmc.PrivateUsage, (unsigned long long)pmc.PeakPagefileUsage,
             pmc.PageFaultCount, handles,
             (unsigned long long)sys.ullAvailPhys, (unsigned long long)sys.ullTotalPhys,
             (long long)g_saveBytes, g_saveWrites, g_saveFailures);
    return b;
}

// Saves the game had open and never closed, for the crash report: "it died
// while writing save3.hg" is worth more than the whole stack walk.
std::string InFlightSavesReport() {
    std::string out;
    AcquireSRWLockShared(&g_saveLock);
    for (int i = 0; i < kSaveSlots; ++i) {
        if (!g_saves[i].handle || g_saves[i].writes == 0) continue;
        char b[192];
        snprintf(b, sizeof(b), "    %s: %lld bytes in %ld writes, not finished\n",
                 ToUtf8(g_saves[i].name).c_str(), (long long)g_saves[i].bytes, g_saves[i].writes);
        out += b;
    }
    ReleaseSRWLockShared(&g_saveLock);
    return out;
}

// On the way out, a save still open never got its closing line. Reported as a
// warning rather than as the usual note, because a save the game was still
// writing when it exited is a save that may be short.
void ReportUnfinishedSaves() {
    AcquireSRWLockExclusive(&g_saveLock);
    for (int i = 0; i < kSaveSlots; ++i) {
        if (!g_saves[i].handle || g_saves[i].writes == 0) continue;
        logger::Pushf(Level::Warn, "save", "%s was still open when the game ended: %lld bytes in %ld writes",
                      ToUtf8(g_saves[i].name).c_str(), (long long)g_saves[i].bytes, g_saves[i].writes);
        g_saves[i].handle = nullptr;
        g_saves[i].writes = 0;
    }
    ReleaseSRWLockExclusive(&g_saveLock);
}

// Periodic status line: counters prove the file hooks are still receiving
// calls, and the byte check detects another component un-patching them.
std::string HeartbeatReport() {
    auto patched = [](const char* fn) {
        auto p = (const unsigned char*)GetProcAddress(GetModuleHandleW(L"ntdll"), fn);
        return p && *p == 0xE9;   // MinHook writes a JMP rel32 at the entry point
    };
    char b[320];
    snprintf(b, sizeof(b),
             "heartbeat: %lld file opens seen (%lld under GAMEDATA); hooks %s; saves %ld writes, %.1f MB%s",
             (long long)g_openCount, (long long)g_gameDataOpens,
             patched("NtCreateFile") && patched("NtOpenFile") ? "intact" : "REMOVED by another component",
             g_saveWrites, (double)g_saveBytes / (1024.0 * 1024.0),
             g_saveFailures ? ", SOME FAILED" : "");
    return b;
}

// user32 may not be mapped yet while our DllMain runs, so these are installed
// from a thread that starts once the loader has finished static imports.
void InstallLate() {
    if (!GetModuleHandleW(L"user32.dll")) LoadLibraryW(L"user32.dll");
    bool a = Hook(L"user32", "MessageBoxA", &hk_MessageBoxA, &o_MessageBoxA);
    bool w = Hook(L"user32", "MessageBoxW", &hk_MessageBoxW, &o_MessageBoxW);
    if (a) MH_EnableHook(GetProcAddress(GetModuleHandleW(L"user32.dll"), "MessageBoxA"));
    if (w) MH_EnableHook(GetProcAddress(GetModuleHandleW(L"user32.dll"), "MessageBoxW"));
}

} // namespace hooks

// Event queue + background writer.
// Every event goes to NMSLogger\hook_latest.log (raw backup that survives
// without the host) and, when the host is running, to the named pipe as a
// JSON line. Events produced before the host connects are kept in a backlog
// and replayed on connect, so starting the host after the game loses nothing.
#include "common.h"
#include <vector>
#include <cstdarg>
#include <cstdio>

namespace {

struct Event {
    FILETIME ft;          // UTC
    DWORD tid;
    Level lvl;
    const char* cat;      // static string
    std::string msg;
};

SRWLOCK g_qLock = SRWLOCK_INIT;
std::vector<Event> g_queue;
SRWLOCK g_ioLock = SRWLOCK_INIT;   // guards file/pipe/backlog
HANDLE g_wake = nullptr;
HANDLE g_thread = nullptr;
volatile LONG g_stop = 0;
volatile LONG g_ready = 0;

HANDLE g_file = INVALID_HANDLE_VALUE;
HANDLE g_pipe = INVALID_HANDLE_VALUE;
std::vector<std::string> g_backlog;
size_t g_backlogDropped = 0;
constexpr size_t kBacklogMax = 100000;
ULONGLONG g_lastPipeAttempt = 0;

const char* LevelName(Level l) {
    switch (l) {
        case Level::Debug: return "DEBUG";
        case Level::Info:  return "INFO ";
        case Level::Warn:  return "WARN ";
        case Level::Error: return "ERROR";
        default:           return "FATAL";
    }
}

BOOL RawWrite(HANDLE h, const std::string& s) {
    if (h == INVALID_HANDLE_VALUE || s.empty()) return TRUE;
    auto wf = hooks::o_WriteFile ? hooks::o_WriteFile : &WriteFile;
    const char* p = s.data();
    size_t left = s.size();
    while (left) {
        DWORD chunk = (DWORD)(left > (1u << 20) ? (1u << 20) : left), done = 0;
        if (!wf(h, p, chunk, &done, nullptr) || done == 0) return FALSE;
        p += done; left -= done;
    }
    return TRUE;
}

void AppendJsonString(std::string& out, const std::string& s) {
    out += '"';
    for (unsigned char c : s) {
        switch (c) {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20) { char b[8]; snprintf(b, sizeof(b), "\\u%04x", c); out += b; }
                else out += (char)c;
        }
    }
    out += '"';
}

long long UnixMs(const FILETIME& ft) {
    ULARGE_INTEGER u; u.LowPart = ft.dwLowDateTime; u.HighPart = ft.dwHighDateTime;
    return (long long)((u.QuadPart - 116444736000000000ULL) / 10000ULL);
}

std::string TextLine(const Event& e) {
    FILETIME local; SYSTEMTIME st;
    FileTimeToLocalFileTime(&e.ft, &local);
    FileTimeToSystemTime(&local, &st);
    char head[96];
    snprintf(head, sizeof(head), "%04u-%02u-%02u %02u:%02u:%02u.%03u [%s] [%-9s] (tid %5lu) ",
             st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds,
             LevelName(e.lvl), e.cat, e.tid);
    std::string line = head;
    line += e.msg;
    line += "\r\n";
    return line;
}

std::string JsonLine(const Event& e) {
    std::string j;
    j.reserve(e.msg.size() + 96);
    char head[96];
    snprintf(head, sizeof(head), "{\"ts\":%lld,\"lvl\":%d,\"tid\":%lu,\"cat\":", UnixMs(e.ft), (int)e.lvl, e.tid);
    j += head;
    AppendJsonString(j, e.cat);
    j += ",\"msg\":";
    AppendJsonString(j, e.msg);
    j += "}\n";
    return j;
}

std::string HelloLine() {
    wchar_t exe[MAX_PATH];
    GetModuleFileNameW(nullptr, exe, MAX_PATH);
    std::string j = "{\"type\":\"hello\",\"pid\":" + std::to_string(GetCurrentProcessId()) +
                    ",\"hookVersion\":\"" NMSLOG_HOOK_VERSION "\",\"exe\":";
    AppendJsonString(j, ToUtf8(exe));
    j += ",\"backlogDropped\":" + std::to_string(g_backlogDropped) + "}\n";
    return j;
}

// Caller holds g_ioLock exclusively.
void TryConnectPipe() {
    if (g_pipe != INVALID_HANDLE_VALUE) return;
    ULONGLONG now = GetTickCount64();
    if (now - g_lastPipeAttempt < 1000) return;
    g_lastPipeAttempt = now;
    auto cf = hooks::o_CreateFileW ? hooks::o_CreateFileW : &CreateFileW;
    HANDLE h = cf(NMSLOG_PIPE_NAME, GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, 0, nullptr);
    if (h == INVALID_HANDLE_VALUE) return;
    g_pipe = h;
    std::string batch = HelloLine();
    for (auto& l : g_backlog) batch += l;
    if (RawWrite(g_pipe, batch)) {
        g_backlog.clear();
        g_backlog.shrink_to_fit();
        g_backlogDropped = 0;
    } else {
        CloseHandle(g_pipe);
        g_pipe = INVALID_HANDLE_VALUE;
    }
}

// Caller holds g_ioLock exclusively.
void WriteEvents(std::vector<Event>& evs, bool allowPipe) {
    if (evs.empty()) return;
    std::string text, json;
    for (auto& e : evs) {
        text += TextLine(e);
        json += JsonLine(e);
    }
    RawWrite(g_file, text);
    if (allowPipe && g_pipe != INVALID_HANDLE_VALUE) {
        if (!RawWrite(g_pipe, json)) {       // host went away: fall back to backlog
            CloseHandle(g_pipe);
            g_pipe = INVALID_HANDLE_VALUE;
            allowPipe = false;
        } else {
            return;
        }
    }
    for (auto& e : evs) {
        if (g_backlog.size() >= kBacklogMax) { ++g_backlogDropped; continue; }
        g_backlog.push_back(JsonLine(e));
    }
}

std::vector<Event> TakeQueue() {
    std::vector<Event> out;
    AcquireSRWLockExclusive(&g_qLock);
    out.swap(g_queue);
    ReleaseSRWLockExclusive(&g_qLock);
    return out;
}

DWORD WINAPI WriterThread(LPVOID) {
    t_inHook = true;   // never observe our own file/pipe I/O
    ULONGLONG nextHeartbeat = GetTickCount64() + 60 * 1000;
    while (!g_stop) {
        WaitForSingleObject(g_wake, 250);
        if (GetTickCount64() >= nextHeartbeat) {
            logger::Push(Level::Debug, "hook", hooks::HeartbeatReport());
            // Same timer, separate line: this one is key=value telemetry the
            // host trends across the session rather than prose for a reader.
            if (g_config.logMemory) logger::Push(Level::Debug, "memory", hooks::MemoryReport());
            nextHeartbeat = GetTickCount64() + 5 * 60 * 1000;
        }
        auto evs = TakeQueue();
        AcquireSRWLockExclusive(&g_ioLock);
        TryConnectPipe();
        WriteEvents(evs, true);
        ReleaseSRWLockExclusive(&g_ioLock);
    }
    return 0;
}

} // namespace

namespace logger {

bool Init() {
    CreateDirectoryW(g_outDir.c_str(), nullptr);
    std::wstring latest = g_outDir + L"hook_latest.log";
    MoveFileExW(latest.c_str(), (g_outDir + L"hook_previous.log").c_str(), MOVEFILE_REPLACE_EXISTING);
    g_file = CreateFileW(latest.c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                         CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    RawWrite(g_file, std::string("\xEF\xBB\xBF"));   // UTF-8 BOM so Notepad shows paths correctly
    g_wake = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    g_thread = CreateThread(nullptr, 0, WriterThread, nullptr, 0, nullptr);
    InterlockedExchange(&g_ready, 1);
    return g_file != INVALID_HANDLE_VALUE;
}

void Push(Level lvl, const char* cat, std::string msg) {
    if (!g_ready) return;
    Event e;
    GetSystemTimePreciseAsFileTime(&e.ft);
    e.tid = GetCurrentThreadId();
    e.lvl = lvl;
    e.cat = cat;
    e.msg = std::move(msg);
    AcquireSRWLockExclusive(&g_qLock);
    g_queue.push_back(std::move(e));
    ReleaseSRWLockExclusive(&g_qLock);
    if (lvl >= Level::Error) SetEvent(g_wake);
}

void Pushf(Level lvl, const char* cat, const char* fmt, ...) {
    char buf[2048];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    Push(lvl, cat, buf);
}

void FlushSync(DWORD timeoutMs) {
    if (!g_ready) return;
    ULONGLONG deadline = GetTickCount64() + timeoutMs;
    // The writer (or a thread killed mid-write) may hold the lock; never block forever.
    bool locked = false;
    while (!(locked = TryAcquireSRWLockExclusive(&g_ioLock)) && GetTickCount64() < deadline)
        Sleep(1);
    std::vector<Event> evs;
    if (TryAcquireSRWLockExclusive(&g_qLock)) {
        evs.swap(g_queue);
        ReleaseSRWLockExclusive(&g_qLock);
    }
    if (locked) {
        WriteEvents(evs, true);
        ReleaseSRWLockExclusive(&g_ioLock);
    } else {
        std::string text;
        for (auto& e : evs) text += TextLine(e);
        RawWrite(g_file, text);
    }
    if (g_file != INVALID_HANDLE_VALUE) FlushFileBuffers(g_file);
}

void Shutdown() {
    if (!g_ready) return;
    InterlockedExchange(&g_stop, 1);
    FlushSync(500);
}

} // namespace logger

// First-chance exception logging (vectored handler) and the crash report
// written from the unhandled-exception filter.
#include "common.h"
#include <cstdio>

namespace crash {
LPTOP_LEVEL_EXCEPTION_FILTER g_gameFilter = nullptr;
}

namespace {

volatile LONG g_crashed = 0;
volatile LONG g_firstChanceLogged = 0;
volatile LONG g_cppLogged = 0;
constexpr LONG kFirstChanceMax = 200;
constexpr LONG kCppMax = 50;

// Tiny lock-free "seen this address N times" table for rate limiting.
constexpr int kAddrSlots = 512;
volatile LONG64 g_addrKeys[kAddrSlots];
volatile LONG g_addrHits[kAddrSlots];

LONG HitCount(const void* addr) {
    LONG64 key = (LONG64)addr;
    for (int i = 0, slot = (int)((key >> 4) % kAddrSlots); i < 16; ++i, slot = (slot + 1) % kAddrSlots) {
        LONG64 cur = g_addrKeys[slot];
        if (cur == key) return InterlockedIncrement(&g_addrHits[slot]);
        if (cur == 0 && InterlockedCompareExchange64(&g_addrKeys[slot], key, 0) == 0)
            return InterlockedIncrement(&g_addrHits[slot]);
    }
    return 1000;   // table saturated around this slot: treat as noisy
}

const char* CodeName(DWORD code) {
    switch (code) {
        case EXCEPTION_ACCESS_VIOLATION:      return "ACCESS_VIOLATION";
        case EXCEPTION_ILLEGAL_INSTRUCTION:   return "ILLEGAL_INSTRUCTION";
        case EXCEPTION_PRIV_INSTRUCTION:      return "PRIVILEGED_INSTRUCTION";
        case EXCEPTION_INT_DIVIDE_BY_ZERO:    return "INTEGER_DIVIDE_BY_ZERO";
        case EXCEPTION_INT_OVERFLOW:          return "INTEGER_OVERFLOW";
        case EXCEPTION_STACK_OVERFLOW:        return "STACK_OVERFLOW";
        case EXCEPTION_IN_PAGE_ERROR:         return "IN_PAGE_ERROR (file/disk read failed)";
        case EXCEPTION_ARRAY_BOUNDS_EXCEEDED: return "ARRAY_BOUNDS_EXCEEDED";
        case EXCEPTION_DATATYPE_MISALIGNMENT: return "DATATYPE_MISALIGNMENT";
        case EXCEPTION_BREAKPOINT:            return "BREAKPOINT (assert?)";
        case 0xC0000374:                      return "HEAP_CORRUPTION";
        case 0xC0000409:                      return "STACK_BUFFER_OVERRUN / fail-fast";
        case 0xE06D7363:                      return "C++ exception";
        case 0xC0000017:                      return "NO_MEMORY";
        default:                              return "unknown";
    }
}

bool IsSerious(DWORD code) {
    switch (code) {
        case EXCEPTION_ACCESS_VIOLATION: case EXCEPTION_ILLEGAL_INSTRUCTION: case EXCEPTION_PRIV_INSTRUCTION:
        case EXCEPTION_INT_DIVIDE_BY_ZERO: case EXCEPTION_INT_OVERFLOW: case EXCEPTION_STACK_OVERFLOW:
        case EXCEPTION_IN_PAGE_ERROR: case EXCEPTION_ARRAY_BOUNDS_EXCEEDED: case EXCEPTION_DATATYPE_MISALIGNMENT:
        case EXCEPTION_BREAKPOINT: case 0xC0000374: case 0xC0000409: case 0xC0000017:
            return true;
        default:
            return false;
    }
}

std::string AccessDetail(const EXCEPTION_RECORD* er) {
    if ((er->ExceptionCode != EXCEPTION_ACCESS_VIOLATION && er->ExceptionCode != EXCEPTION_IN_PAGE_ERROR) ||
        er->NumberParameters < 2)
        return {};
    const char* op = er->ExceptionInformation[0] == 0 ? "reading" : er->ExceptionInformation[0] == 1 ? "writing" : "executing";
    char b[96];
    snprintf(b, sizeof(b), " (%s address 0x%llX)", op, (unsigned long long)er->ExceptionInformation[1]);
    return b;
}

// Walks the x64 stack from a context using the unwind tables. SEH-guarded,
// so it only touches raw arrays (no C++ objects needing unwinding).
int WalkStack(CONTEXT ctx, DWORD64* frames, int max) {
    int n = 0;
    __try {
        while (n < max && ctx.Rip) {
            frames[n++] = ctx.Rip;
            DWORD64 imageBase = 0;
            PRUNTIME_FUNCTION fe = RtlLookupFunctionEntry(ctx.Rip, &imageBase, nullptr);
            if (!fe) {                       // leaf function: return address is at [rsp]
                ctx.Rip = *(DWORD64*)ctx.Rsp;
                ctx.Rsp += 8;
            } else {
                PVOID handlerData = nullptr;
                DWORD64 establisher = 0;
                RtlVirtualUnwind(UNW_FLAG_NHANDLER, imageBase, ctx.Rip, fe, &ctx, &handlerData, &establisher, nullptr);
            }
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
    }
    return n;
}

std::string BuildReport(EXCEPTION_POINTERS* ep, DWORD tid) {
    const EXCEPTION_RECORD* er = ep->ExceptionRecord;
    const CONTEXT* c = ep->ContextRecord;
    std::string r;
    char b[512];
    snprintf(b, sizeof(b), "UNHANDLED EXCEPTION - the game is crashing\n  exception: 0x%08lX %s%s\n  location:  %s\n  thread:    %lu\n",
             er->ExceptionCode, CodeName(er->ExceptionCode), AccessDetail(er).c_str(),
             DescribeAddress(er->ExceptionAddress).c_str(), tid);
    r += b;
    snprintf(b, sizeof(b),
             "  registers: RAX=%016llX RBX=%016llX RCX=%016llX RDX=%016llX\n"
             "             RSI=%016llX RDI=%016llX RBP=%016llX RSP=%016llX\n"
             "             R8 =%016llX R9 =%016llX R10=%016llX R11=%016llX\n"
             "             R12=%016llX R13=%016llX R14=%016llX R15=%016llX\n",
             c->Rax, c->Rbx, c->Rcx, c->Rdx, c->Rsi, c->Rdi, c->Rbp, c->Rsp,
             c->R8, c->R9, c->R10, c->R11, c->R12, c->R13, c->R14, c->R15);
    r += b;
    DWORD64 frames[64];
    int n = WalkStack(*c, frames, 64);
    r += "  stack:\n";
    for (int i = 0; i < n; ++i) {
        snprintf(b, sizeof(b), "    #%02d %s\n", i, DescribeAddress((void*)frames[i]).c_str());
        r += b;
    }
    // Before the file list, because it is the one line here that can change what
    // the player does next: a save that was being written when the game died may
    // be short, and the copy from before this session is the one to keep.
    std::string saves = hooks::InFlightSavesReport();
    if (!saves.empty()) {
        r += "  A SAVE WAS BEING WRITTEN WHEN THIS HAPPENED:\n";
        r += saves;
    }
    r += "  most recent file opens (newest first):\n";
    r += hooks::RecentFilesReport();
    return r;
}

struct ReportJob { EXCEPTION_POINTERS* ep; DWORD tid; };

// Runs on a fresh thread so a stack overflow on the crashing thread doesn't
// stop us from formatting the report.
DWORD WINAPI ReportThread(LPVOID p) {
    t_inHook = true;
    auto* job = (ReportJob*)p;
    logger::Push(Level::Fatal, "crash", BuildReport(job->ep, job->tid));
    logger::FlushSync(3000);
    return 0;
}

LONG CALLBACK VectoredHandler(EXCEPTION_POINTERS* ep) {
    if (t_inHook || !g_config.logFirstChanceExceptions) return EXCEPTION_CONTINUE_SEARCH;
    DWORD code = ep->ExceptionRecord->ExceptionCode;
    void* addr = ep->ExceptionRecord->ExceptionAddress;
    if (code == EXCEPTION_STACK_OVERFLOW) return EXCEPTION_CONTINUE_SEARCH;   // reported by the filter
    if (code == 0xE06D7363) {
        if (g_cppLogged >= kCppMax || HitCount(addr) > 3) return EXCEPTION_CONTINUE_SEARCH;
        InterlockedIncrement(&g_cppLogged);
        ReentryGuard g;
        logger::Pushf(Level::Debug, "exception", "C++ exception thrown (first-chance) near %s",
                      DescribeAddress(addr).c_str());
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if (!IsSerious(code)) return EXCEPTION_CONTINUE_SEARCH;
    if (g_firstChanceLogged >= kFirstChanceMax || HitCount(addr) > 3) return EXCEPTION_CONTINUE_SEARCH;
    InterlockedIncrement(&g_firstChanceLogged);
    ReentryGuard g;
    logger::Pushf(Level::Warn, "exception", "first-chance 0x%08lX %s%s at %s (may be handled by the game)",
                  code, CodeName(code), AccessDetail(ep->ExceptionRecord).c_str(), DescribeAddress(addr).c_str());
    return EXCEPTION_CONTINUE_SEARCH;
}

} // namespace

namespace crash {

LONG WINAPI UnhandledFilter(EXCEPTION_POINTERS* ep) {
    if (InterlockedExchange(&g_crashed, 1) == 0) {
        ReportJob job{ep, GetCurrentThreadId()};
        HANDLE t = CreateThread(nullptr, 256 * 1024, ReportThread, &job, 0, nullptr);
        if (t) {
            WaitForSingleObject(t, 5000);
            CloseHandle(t);
        } else {
            logger::Pushf(Level::Fatal, "crash", "UNHANDLED EXCEPTION 0x%08lX at %s",
                          ep->ExceptionRecord->ExceptionCode, DescribeAddress(ep->ExceptionRecord->ExceptionAddress).c_str());
            logger::FlushSync(2000);
        }
    }
    if (g_gameFilter) {
        LONG r = g_gameFilter(ep);   // lets NMS write its own NMS_crash_*.dmp
        logger::Pushf(Level::Info, "crash", "game crash handler returned %ld", r);
        logger::FlushSync(1000);
        return r;
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

void Install() {
    AddVectoredExceptionHandler(1, VectoredHandler);
    auto set = hooks::o_SetUnhandledExceptionFilter ? hooks::o_SetUnhandledExceptionFilter : &SetUnhandledExceptionFilter;
    LPTOP_LEVEL_EXCEPTION_FILTER prev = set(UnhandledFilter);
    if (prev && prev != UnhandledFilter) g_gameFilter = prev;
}

} // namespace crash

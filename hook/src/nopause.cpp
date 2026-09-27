// Keep the game running when it loses focus.
//
// WHY THIS ONE IS DIFFERENT
// ------------------------
// No Man's Sky pauses when its window is deactivated and offers no setting to stop
// it. Fixing that needs nothing from the game's own code: no offsets, no class
// metadata, no scanning. It is the same kind of work as every other hook in this
// DLL -- intercepting the OS boundary -- which is why it survives game patches
// while anything built on struct offsets does not.
//
// It is also the first thing here shaped like a plugin: one self-contained
// behaviour, one config flag, no dependency on the metadata work. When the plugin
// host exists this should move out of the DLL unchanged.
//
// HOW
// ---
// Two layers, because a game can notice deactivation either way and there is no
// way to tell from outside which one NMS uses:
//
//   1. the message pump. WM_ACTIVATEAPP / WM_ACTIVATE carry a flag saying
//      "going inactive"; that flag is flipped. WM_KILLFOCUS has nothing to flip,
//      so it becomes WM_NULL, which every message loop already ignores.
//   2. the polling calls. GetForegroundWindow, GetActiveWindow and GetFocus
//      answer with our own window when the real answer is somebody else's.
//
// Messages are rewritten rather than dropped. Returning FALSE from PeekMessage
// when a message really is waiting risks a loop that spins or stalls, and that is
// a worse bug than the one being fixed.
//
// WHAT IT COSTS
// -------------
// The game believes it is focused when it is not, which is the entire point, but
// the consequences reach further than pausing: it may keep grabbing the cursor, and
// it will keep rendering and burning GPU while you are in another window. Off by
// default, and it says so in the log when it is on.
#include "common.h"
#include "../third_party/minhook/include/MinHook.h"

namespace {

decltype(&GetForegroundWindow) o_GetForegroundWindow = nullptr;
decltype(&GetActiveWindow)     o_GetActiveWindow = nullptr;
decltype(&GetFocus)            o_GetFocus = nullptr;
decltype(&PeekMessageW)        o_PeekMessageW = nullptr;
decltype(&PeekMessageA)        o_PeekMessageA = nullptr;
decltype(&GetMessageW)         o_GetMessageW = nullptr;
decltype(&GetMessageA)         o_GetMessageA = nullptr;

HWND          g_mainWindow = nullptr;
volatile LONG g_suppressed = 0;      // how many deactivations we have hidden
volatile LONG g_logged = 0;

BOOL CALLBACK PickWindow(HWND h, LPARAM param) {
    DWORD pid = 0;
    GetWindowThreadProcessId(h, &pid);
    if (pid != GetCurrentProcessId()) return TRUE;
    if (!IsWindowVisible(h)) return TRUE;
    if (GetWindow(h, GW_OWNER) != nullptr) return TRUE;   // not a top-level window
    RECT r{};
    if (!GetWindowRect(h, &r)) return TRUE;
    if (r.right - r.left < 200 || r.bottom - r.top < 200) return TRUE;
    *(HWND*)param = h;
    return FALSE;
}

// The game's main window, found once and cached. Enumerating on every hooked call
// would be absurd, and these hooks are on the per-frame path.
HWND MainWindow() {
    if (g_mainWindow && IsWindow(g_mainWindow)) return g_mainWindow;
    HWND found = nullptr;
    EnumWindows(PickWindow, (LPARAM)&found);
    g_mainWindow = found;
    return found;
}

void NoteSuppressed(const char* what) {
    LONG n = InterlockedIncrement(&g_suppressed);
    // Once, so a four-hour session does not carry four hours of this, but loud
    // enough that "did it actually engage?" is answerable from the log.
    if (InterlockedCompareExchange(&g_logged, 1, 0) == 0)
        logger::Pushf(Level::Info, "nopause",
                      "suppressed the first focus-loss (%s); the game will keep running "
                      "in the background from here", what);
    else if (n % 500 == 0)
        logger::Pushf(Level::Debug, "nopause", "suppressed %ld focus-loss events", n);
}

// Rewrites one message in place. Shared by all four pump hooks.
void Defuse(MSG* m) {
    if (!m) return;
    switch (m->message) {
        case WM_ACTIVATEAPP:
            if (m->wParam == FALSE) { m->wParam = TRUE; NoteSuppressed("WM_ACTIVATEAPP"); }
            break;
        case WM_ACTIVATE:
            if (LOWORD(m->wParam) == WA_INACTIVE) {
                m->wParam = MAKELONG(WA_ACTIVE, HIWORD(m->wParam));
                NoteSuppressed("WM_ACTIVATE");
            }
            break;
        case WM_KILLFOCUS:
            m->message = WM_NULL;       // nothing to flip; make it a no-op instead
            m->wParam = 0;
            m->lParam = 0;
            NoteSuppressed("WM_KILLFOCUS");
            break;
        default:
            break;
    }
}

BOOL WINAPI hk_PeekMessageW(LPMSG m, HWND h, UINT f, UINT l, UINT r) {
    BOOL got = o_PeekMessageW(m, h, f, l, r);
    if (got) Defuse(m);
    return got;
}
BOOL WINAPI hk_PeekMessageA(LPMSG m, HWND h, UINT f, UINT l, UINT r) {
    BOOL got = o_PeekMessageA(m, h, f, l, r);
    if (got) Defuse(m);
    return got;
}
BOOL WINAPI hk_GetMessageW(LPMSG m, HWND h, UINT f, UINT l) {
    BOOL got = o_GetMessageW(m, h, f, l);
    if (got > 0) Defuse(m);
    return got;
}
BOOL WINAPI hk_GetMessageA(LPMSG m, HWND h, UINT f, UINT l) {
    BOOL got = o_GetMessageA(m, h, f, l);
    if (got > 0) Defuse(m);
    return got;
}

// Answer with our own window only when the truthful answer is somebody else's.
// Always returning it would lie even while the game IS focused, which breaks
// nothing visibly but makes any other focus logic in the process untrustworthy.
HWND WINAPI hk_GetForegroundWindow() {
    HWND real = o_GetForegroundWindow();
    HWND mine = MainWindow();
    if (mine && real != mine) return mine;
    return real;
}
HWND WINAPI hk_GetActiveWindow() {
    HWND real = o_GetActiveWindow();
    if (real) return real;
    HWND mine = MainWindow();
    return mine ? mine : real;
}
HWND WINAPI hk_GetFocus() {
    HWND real = o_GetFocus();
    if (real) return real;
    HWND mine = MainWindow();
    return mine ? mine : real;
}

// Create and enable in one step: this runs after hooks::Install() has already
// called MH_EnableHook(MH_ALL_HOOKS), so a newly created hook would otherwise sit
// there disabled and silently do nothing.
template <typename T>
bool HookNow(const char* fn, void* detour, T* orig) {
    if (MH_CreateHookApi(L"user32", fn, detour, reinterpret_cast<void**>(orig)) != MH_OK) {
        logger::Pushf(Level::Warn, "nopause", "could not hook user32!%s", fn);
        return false;
    }
    void* target = (void*)GetProcAddress(GetModuleHandleW(L"user32.dll"), fn);
    if (!target || MH_EnableHook(target) != MH_OK) {
        logger::Pushf(Level::Warn, "nopause", "hooked but could not enable user32!%s", fn);
        return false;
    }
    return true;
}

} // namespace

namespace nopause {

void Install() {
    if (!g_config.noPauseOnFocusLoss) return;
    if (!GetModuleHandleW(L"user32.dll")) LoadLibraryW(L"user32.dll");

    int ok = 0;
    ok += HookNow("PeekMessageW", &hk_PeekMessageW, &o_PeekMessageW);
    ok += HookNow("PeekMessageA", &hk_PeekMessageA, &o_PeekMessageA);
    ok += HookNow("GetMessageW", &hk_GetMessageW, &o_GetMessageW);
    ok += HookNow("GetMessageA", &hk_GetMessageA, &o_GetMessageA);
    ok += HookNow("GetForegroundWindow", &hk_GetForegroundWindow, &o_GetForegroundWindow);
    ok += HookNow("GetActiveWindow", &hk_GetActiveWindow, &o_GetActiveWindow);
    ok += HookNow("GetFocus", &hk_GetFocus, &o_GetFocus);

    HWND w = MainWindow();
    logger::Pushf(Level::Info, "nopause",
                  "no-pause-on-focus-loss is ON: %d of 7 hooks installed, main window %p. "
                  "The game will keep rendering and may keep the cursor while you are in "
                  "another window.",
                  ok, (void*)w);
}

} // namespace nopause

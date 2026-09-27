// xinput9_1_0.dll proxy: forwards every export to the real System32 DLL.
// NMS.exe imports XInputGetState/XInputSetState from this DLL, which is how
// Windows loads us into the game without an injector.
#include "common.h"

namespace {

HMODULE g_real = nullptr;
INIT_ONCE g_once = INIT_ONCE_STATIC_INIT;

BOOL CALLBACK LoadReal(PINIT_ONCE, PVOID, PVOID*) {
    wchar_t path[MAX_PATH];
    UINT n = GetSystemDirectoryW(path, MAX_PATH);
    if (n && n < MAX_PATH - 20) {
        wcscat_s(path, L"\\xinput9_1_0.dll");
        g_real = LoadLibraryW(path);
    }
    return TRUE;
}

FARPROC Real(const char* name) {
    InitOnceExecuteOnce(&g_once, LoadReal, nullptr, nullptr);
    return g_real ? GetProcAddress(g_real, name) : nullptr;
}

template <typename Fn>
Fn Resolve(Fn& cache, const char* name) {
    if (!cache) cache = reinterpret_cast<Fn>(Real(name));
    return cache;
}

} // namespace

extern "C" {

DWORD WINAPI Proxy_XInputGetState(DWORD user, void* state) {
    static DWORD(WINAPI* fn)(DWORD, void*) = nullptr;
    return Resolve(fn, "XInputGetState") ? fn(user, state) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD WINAPI Proxy_XInputSetState(DWORD user, void* vibration) {
    static DWORD(WINAPI* fn)(DWORD, void*) = nullptr;
    return Resolve(fn, "XInputSetState") ? fn(user, vibration) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD WINAPI Proxy_XInputGetCapabilities(DWORD user, DWORD flags, void* caps) {
    static DWORD(WINAPI* fn)(DWORD, DWORD, void*) = nullptr;
    return Resolve(fn, "XInputGetCapabilities") ? fn(user, flags, caps) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD WINAPI Proxy_XInputGetDSoundAudioDeviceGuids(DWORD user, GUID* render, GUID* capture) {
    static DWORD(WINAPI* fn)(DWORD, GUID*, GUID*) = nullptr;
    return Resolve(fn, "XInputGetDSoundAudioDeviceGuids") ? fn(user, render, capture) : ERROR_DEVICE_NOT_CONNECTED;
}

// Lets the host installer identify this DLL.
const char* WINAPI NMSLoggerHookVersion() { return NMSLOG_MARKER " " NMSLOG_HOOK_VERSION; }

}

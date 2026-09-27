// Test stand-in for NMS.exe: imports xinput9_1_0.dll like the game does and
// exercises every hook. Build with hook\test\run_test.ps1.
#include <windows.h>
#include <shlobj.h>     // SHCreateDirectoryExW, for the fake save folder
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

extern "C" DWORD WINAPI XInputGetState(DWORD, void*);

// Minimal NT structures (avoids winternl.h in the test).
struct NT_USTR { USHORT Length, MaximumLength; PWSTR Buffer; };
struct NT_OA { ULONG Length; HANDLE RootDirectory; NT_USTR* ObjectName; ULONG Attributes; PVOID Sd, Qos; };
typedef NT_OA* PNT_OA;
struct NT_IOSB { ULONG_PTR Status; ULONG_PTR Information; };
typedef NT_IOSB* PNT_IOSB;

// ---------------------------------------------------------------------------
// A synthetic metadata table: the positive control for metaprobe.cpp.
//
// Without this, a "found nothing" from the real game is ambiguous between "the
// game has no such table" and "the probe is broken", which are the two answers
// it most matters to tell apart. So the fake game carries a table the probe is
// obliged to find: records of a known size (0x28) holding a pointer to a real
// NMS class name at offset 0, and a member sub-table of known size (0x10) whose
// records start with a field name.
//
// Two copies on purpose -- one static, one on the heap -- because the probe
// treats an image hit and a private-memory hit as different findings with
// different consequences, and both paths deserve exercising.
struct FakeMember {           // 0x10 bytes
    const char* name;
    unsigned    offset;
    unsigned    type;
};
struct FakeClass {            // 0x28 bytes
    const char*        name;
    unsigned long long guid;
    unsigned           size;
    unsigned           memberCount;
    const FakeMember*  members;
    unsigned long long reserved;
};

static const FakeMember kPlayerMembers[] = {
    {"Health", 0x00, 1}, {"Shield", 0x04, 1}, {"Units", 0x08, 2},
    {"Nanites", 0x0C, 2}, {"Inventory", 0x10, 7}, {"ShipInventory", 0x18, 7},
};
static const FakeMember kContainerMembers[] = {
    {"Slots", 0x00, 7}, {"Width", 0x08, 1}, {"Height", 0x0C, 1},
    {"ValidSlotIndices", 0x10, 7}, {"Class", 0x20, 3},
};
static const FakeMember kSubstanceMembers[] = {
    {"Id", 0x00, 4}, {"Name", 0x10, 4}, {"NameLower", 0x20, 4},
    {"Symbol", 0x30, 4}, {"Colour", 0x40, 5}, {"Category", 0x50, 3},
};

static const FakeClass kFakeMetaTable[] = {
    {"cGcPlayerStateData",      0x1111111111111111ull, 0x2A00, 6, kPlayerMembers,    0},
    {"cGcInventoryContainer",   0x2222222222222222ull, 0x0120, 5, kContainerMembers, 0},
    {"cGcInventoryLayout",      0x3333333333333333ull, 0x0040, 5, kContainerMembers, 0},
    {"cGcRealitySubstanceData", 0x4444444444444444ull, 0x00C0, 6, kSubstanceMembers, 0},
    {"cGcDebugOptions",         0x5555555555555555ull, 0x0800, 6, kPlayerMembers,    0},
    {"cGcSolarSystemData",      0x6666666666666666ull, 0x0600, 6, kSubstanceMembers, 0},
    {"cTkMetaDataClass",        0x7777777777777777ull, 0x0028, 5, kContainerMembers, 0},
    {"cGcProductData",          0x8888888888888888ull, 0x0180, 6, kSubstanceMembers, 0},
};

// Leaked deliberately: the probe has to find it in private memory while the
// game is still running, which is exactly the real-game case that matters.
static void PublishFakeMetaTable() {
    const size_t bytes = sizeof(kFakeMetaTable);
    void* heapCopy = malloc(bytes);
    if (heapCopy) memcpy(heapCopy, kFakeMetaTable, bytes);
    char msg[256];
    sprintf_s(msg, "fake metadata table: %zu records of 0x%zX bytes, static at %p, heap at %p\n",
              sizeof(kFakeMetaTable) / sizeof(kFakeMetaTable[0]), sizeof(FakeClass),
              (const void*)kFakeMetaTable, heapCopy);
    OutputDebugStringA(msg);
}
// ---------------------------------------------------------------------------

static LONG WINAPI GameFilter(EXCEPTION_POINTERS*) {
    OutputDebugStringA("fake game crash filter ran\n");
    return EXCEPTION_EXECUTE_HANDLER;
}

static void HandledAccessViolation() {
    __try { *(volatile int*)8 = 1; } __except (EXCEPTION_EXECUTE_HANDLER) {}
}

int wmain(int argc, wchar_t** argv) {
    SetUnhandledExceptionFilter(GameFilter);   // like NMS registering its dump writer
    PublishFakeMetaTable();
    char state[16] = {};
    XInputGetState(0, state);

    wchar_t exe[MAX_PATH];
    GetModuleFileNameW(nullptr, exe, MAX_PATH);
    *wcsrchr(exe, L'\\') = 0;                    // ...\Binaries
    *wcsrchr(exe, L'\\') = 0;                    // game root
    wchar_t p[MAX_PATH];

    swprintf_s(p, L"%s\\GAMEDATA\\MODS\\TestMod\\METADATA\\REAL.MBIN", exe);
    HANDLE h = CreateFileW(p, GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
    if (h != INVALID_HANDLE_VALUE) CloseHandle(h);
    // NMS style: forward slashes, relative to the working directory (Binaries).
    SetCurrentDirectoryW((std::wstring(exe) + L"\\Binaries").c_str());
    h = CreateFileW(L"../GAMEDATA/MODS/TestMod/METADATA/SLASH.MBIN", GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
    if (h != INVALID_HANDLE_VALUE) CloseHandle(h);
    // Open relative to a directory handle (NtCreateFile with RootDirectory).
    {
        swprintf_s(p, L"%s\\GAMEDATA\\MODS\\TestMod", exe);
        HANDLE dir = CreateFileW(p, FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING,
                                 FILE_FLAG_BACKUP_SEMANTICS, nullptr);
        typedef LONG(NTAPI * NtCreateFile_t)(PHANDLE, ACCESS_MASK, PNT_OA, PNT_IOSB, PLARGE_INTEGER, ULONG, ULONG, ULONG, ULONG, PVOID, ULONG);
        auto ntCreate = (NtCreateFile_t)GetProcAddress(GetModuleHandleW(L"ntdll"), "NtCreateFile");
        wchar_t rel[] = L"METADATA\\RELATIVE.MBIN";
        NT_USTR name{(USHORT)(wcslen(rel) * 2), (USHORT)sizeof(rel), rel};
        NT_OA oa{sizeof(NT_OA), dir, &name, 0x40 /*OBJ_CASE_INSENSITIVE*/, nullptr, nullptr};
        NT_IOSB iosb{};
        HANDLE f2 = nullptr;
        ntCreate(&f2, GENERIC_READ | SYNCHRONIZE, &oa, &iosb, nullptr, 0, FILE_SHARE_READ, 1 /*FILE_OPEN*/,
                 0x20 /*FILE_SYNCHRONOUS_IO_NONALERT*/ | 0x40 /*FILE_NON_DIRECTORY_FILE*/, nullptr, 0);
        if (f2) CloseHandle(f2);
        CloseHandle(dir);
    }
    swprintf_s(p, L"%s\\GAMEDATA\\MODS\\TestMod\\METADATA\\MISSING.MBIN", exe);
    CreateFileW(p, GENERIC_READ, 0, nullptr, OPEN_EXISTING, 0, nullptr);

    swprintf_s(p, L"%s\\GAMEDATA\\FullLog.txt", exe);
    FILE* f = _wfopen(p, L"w");
    fprintf(f, "cTkStorageTemp (PC) unable to load file (Error 00000002) - C:\\fake\\DISABLEMODS\n");
    fprintf(f, "Loaded some things fine\n");
    fclose(f);
    OutputDebugStringA("Loaded some things fine\n");   // NMS echoes log lines here too

    OutputDebugStringA("hello from the fake game\n");
    OutputDebugStringW(L"wide debug: failed to find shader\n");

    // A save, written the way the game writes one: several writes to a file under
    // HelloGames\NMS, then closed. The path only has to *contain* that folder, so
    // nothing here goes near the real save folder.
    {
        wchar_t saves[MAX_PATH];
        swprintf_s(saves, L"%s\\AppData\\Roaming\\HelloGames\\NMS\\st_fake", exe);
        SHCreateDirectoryExW(nullptr, saves, nullptr);
        swprintf_s(p, L"%s\\save3.hg", saves);
        HANDLE save = CreateFileW(p, GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, 0, nullptr);
        if (save != INVALID_HANDLE_VALUE) {
            std::string block(64 * 1024, 'H');
            DWORD wrote = 0;
            for (int i = 0; i < 12; ++i) WriteFile(save, block.data(), (DWORD)block.size(), &wrote, nullptr);
            CloseHandle(save);   // the moment the save is finished
        }
        // And one that cannot be written: opened for reading only, so every
        // write fails the way a full disk or a locked file would.
        swprintf_s(p, L"%s\\mf_save3.hg", saves);
        HANDLE ro = CreateFileW(p, GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, 0, nullptr);
        if (ro != INVALID_HANDLE_VALUE) CloseHandle(ro);
        ro = CreateFileW(p, GENERIC_READ, 0, nullptr, OPEN_EXISTING, 0, nullptr);
        if (ro != INVALID_HANDLE_VALUE) {
            DWORD wrote = 0;
            WriteFile(ro, "nope", 4, &wrote, nullptr);   // ERROR_ACCESS_DENIED
            CloseHandle(ro);
        }
    }

    HandledAccessViolation();   // handled first-chance AV

    Sleep(1500);
    if (argc > 1 && wcscmp(argv[1], L"long") == 0) Sleep(65000);   // long enough for a heartbeat
    if (argc > 1 && wcscmp(argv[1], L"probe") == 0) Sleep(25000);   // long enough for the metadata probe
    if (argc > 1 && wcscmp(argv[1], L"crash") == 0) {
        volatile int* bad = nullptr;
        *bad = 42;
    }
    return 7;
}

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

// ---------------------------------------------------------------------------
// A synthetic inventory: the positive control for the instance probe.
//
// Built byte by byte at measured offsets rather than as a C struct, because the
// point is to reproduce the game's layout exactly -- a compiler is free to pad a
// struct differently, and then a passing test would prove nothing about the real
// thing. The offsets are the ones tools/nms_meta_extract.py read out of NMS.exe.
//
// cGcInventoryElement, 0x30 as an array element:
//   +0x00 Id[16]  +0x10 X  +0x14 Y  +0x18 Amount  +0x1C DamageFactor
//   +0x20 MaxAmount  +0x24 Type  +0x28 AddedAutomatically  +0x29 FullyInstalled
// cGcInventoryContainer, 0x159:
//   +0x10 Slots handle (ptr, then count)  +0x40 Class  +0x44 Height
//   +0x4C StackSizeGroup  +0x50 Version  +0x54 Width  +0x58 Name[256]  +0x158 IsCool
static void PublishFakeInventory() {
    const size_t kElem = 0x30, kCont = 0x159, kSlots = 10;

    unsigned char* slots = (unsigned char*)calloc(kSlots, kElem);
    if (!slots) return;

    struct Seed { const char* id; int x, y, amount, maxAmount; unsigned type; float dmg; };
    // Two empty slots on purpose: a real inventory has them, and the probe has to
    // grow a run across them rather than stopping at the first blank.
    static const Seed seeds[] = {
        {"CARBON",       0, 0,  250, 9999, 1, 0.0f},
        {"OXYGEN",       1, 0,   48, 9999, 1, 0.0f},
        {nullptr,        2, 0,    0,    0, 0, 0.0f},
        {"^LAUNCHFUEL",  3, 0,    7,  100, 2, 0.25f},
        {"FERRITE_DUST", 4, 0, 1200, 9999, 1, 0.0f},
        {nullptr,        5, 0,    0,    0, 0, 0.0f},
        {"TRITIUM",      6, 0,  999, 9999, 1, 0.0f},
        {"CHROMATIC",    7, 0,   12,  250, 1, 0.5f},
        {"SUNRISE",      8, 0,    1,    5, 2, 1.0f},
        {"GOLD",         9, 0,  500, 9999, 1, 0.0f},
    };
    for (size_t k = 0; k < kSlots; ++k) {
        unsigned char* e = slots + k * kElem;
        const Seed& sd = seeds[k];
        if (sd.id) {
            size_t n = strlen(sd.id);
            if (n > 15) n = 15;
            memcpy(e + 0x00, sd.id, n);            // rest stays NUL from calloc
            memcpy(e + 0x18, &sd.amount, 4);
            memcpy(e + 0x1C, &sd.dmg, 4);
            memcpy(e + 0x20, &sd.maxAmount, 4);
            memcpy(e + 0x24, &sd.type, 4);
            e[0x28] = (unsigned char)(k % 2);
            e[0x29] = 1;
        }
        memcpy(e + 0x10, &sd.x, 4);
        memcpy(e + 0x14, &sd.y, 4);
    }

    unsigned char* cont = (unsigned char*)calloc(1, kCont);
    if (!cont) return;
    unsigned long long slotsPtr = (unsigned long long)(void*)slots;
    unsigned int count = (unsigned int)kSlots, cls = 3, ssg = 1, version = 4;
    int width = 10, height = 1;
    memcpy(cont + 0x10, &slotsPtr, 8);
    memcpy(cont + 0x18, &count, 4);
    memcpy(cont + 0x40, &cls, 4);
    memcpy(cont + 0x44, &height, 4);
    memcpy(cont + 0x4C, &ssg, 4);
    memcpy(cont + 0x50, &version, 4);
    memcpy(cont + 0x54, &width, 4);
    const char* nm = "FakeFreighterStorage4";
    memcpy(cont + 0x58, nm, strlen(nm));
    cont[0x158] = 1;

    char msg[256];
    sprintf_s(msg, "fake inventory: %zu slots at %p, container at %p\n",
              kSlots, (void*)slots, (void*)cont);
    OutputDebugStringA(msg);
    // Leaked on purpose: the probe has to find it while the process is alive.
}
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// A synthetic id pool: the negative shape for the string hunt.
//
// The real game holds a table of every substance id in existence, 0x10 apart
// with nothing in between, present whether or not the player owns any of them.
// The first hunt found only these, reported "ids are text on the heap", and that
// read as progress -- it is not, because a pool says nothing about inventories.
// The hunt now classifies each hit by the stride of its id neighbours, so the
// control has to contain both shapes: this pool at 0x10, and the inventory above
// at 0x30. A fixture with only one of them could not catch a probe that confuses
// them, which is the exact failure being guarded against.
//
// The ids are deliberately nonsense: a real id would also appear in the fake
// inventory or in this file's own literals, and then a hit could not be pinned
// to one structure.
static void PublishFakeIdPool() {
    static const char* kPool[] = {
        "ZZPOOL1", "ZZPOOL2", "ZZPOOL3", "ZZPOOL4",
        "ZZPOOL5", "ZZPOOL6", "ZZPOOL7", "ZZPOOL8",
    };
    const size_t n = sizeof(kPool) / sizeof(kPool[0]);
    unsigned char* pool = (unsigned char*)calloc(n, 0x10);
    if (!pool) return;
    for (size_t k = 0; k < n; ++k)
        memcpy(pool + k * 0x10, kPool[k], strlen(kPool[k]));   // NUL padding from calloc
    char msg[128];
    sprintf_s(msg, "fake id pool: %zu ids at 0x10 stride, at %p\n", n, (void*)pool);
    OutputDebugStringA(msg);
    // Leaked on purpose, like the other fixtures.
}
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// A synthetic cGcPlayerStateData: the positive control for phase P.
//
// 27 containers of 0x160 bytes at offset 0x809D8 inside one allocation, which is
// where tools/nms_meta_extract.py says they live and how far apart. Phase P looks
// for that periodicity rather than for any one container's field values.
//
// WHAT THIS CAN AND CANNOT PROVE. It exercises the scan, the clustering and the
// reporting: if phase P cannot find 27 containers laid out exactly as the offline
// extraction describes, it is broken and a negative result from the real game
// means nothing. It CANNOT confirm that the game's array handle really is
// {pointer, count, capacity} at +0x00/+0x08/+0x0C, because this fixture writes
// handles in that very shape -- the same trap the old fake container fell into,
// where a control built from a hypothesis was read as evidence for it. Only the
// live game can settle the handle layout.
//
// Two containers are left completely empty on purpose: unowned storage chests
// really are empty, and phase P must not need every member of the run to be
// populated.
static void PublishFakePlayerState() {
    const size_t kCont = 0x160, kRun = 27, kFirst = 0x809D8, kClass = 0x86A11;
    unsigned char* psd = (unsigned char*)calloc(1, kClass);
    if (!psd) return;

    static const char* kIds[] = { "FUEL1", "YELLOW2", "OXYGEN",
                                  "ASTEROID1", "ASTEROID3", "RED2" };
    for (size_t c = 0; c < kRun; ++c) {
        unsigned char* cont = psd + kFirst + c * kCont;
        if (c == 14 || c == 20) continue;         // FishBaitBox and Grave: empty
        const size_t kSlots = 6;
        unsigned char* slots = (unsigned char*)calloc(kSlots, 0x30);
        if (!slots) continue;
        for (size_t k = 0; k < kSlots; ++k) {
            unsigned char* e = slots + k * 0x30;
            const char* id = kIds[(c + k) % 6];
            memcpy(e + 0x00, id, strlen(id));
            int amount = (int)(100 + c * 10 + k), maxAmount = 9999, x = (int)k;
            unsigned int type = 1;
            memcpy(e + 0x10, &x, 4);
            memcpy(e + 0x18, &amount, 4);
            memcpy(e + 0x20, &maxAmount, 4);
            memcpy(e + 0x24, &type, 4);
        }
        // Slots at +0x10 and ValidSlotIndices at +0x30, each a
        // {pointer, count, capacity} handle. BaseStatValues (+0x00) and
        // SpecialSlots (+0x20) stay zeroed, which is a valid empty handle.
        unsigned long long sp = (unsigned long long)(void*)slots;
        unsigned int n = (unsigned int)kSlots;
        memcpy(cont + 0x10, &sp, 8);
        memcpy(cont + 0x18, &n, 4);
        memcpy(cont + 0x1C, &n, 4);
        unsigned char* valid = (unsigned char*)calloc(kSlots, 8);
        if (valid) {
            unsigned long long vp = (unsigned long long)(void*)valid;
            memcpy(cont + 0x30, &vp, 8);
            memcpy(cont + 0x38, &n, 4);
            memcpy(cont + 0x3C, &n, 4);
        }
        // Width and Height are written as the game writes them -- which is to say
        // unreliably. Container 3 gets 1x1 and container 5 gets 16x1, the two cases
        // the save editor documents as lies, so that any future filter on grid
        // geometry fails this control instead of failing in the real game.
        int w = 10, h = 1;
        if (c == 3) { w = 1; h = 1; }
        if (c == 5) { w = 16; h = 1; }
        unsigned int cls = 3, ver = 4;
        memcpy(cont + 0x40, &cls, 4);
        memcpy(cont + 0x44, &h, 4);
        memcpy(cont + 0x50, &ver, 4);
        memcpy(cont + 0x54, &w, 4);
        char nm[48];
        sprintf_s(nm, "FakeRunContainer%zu", c);
        memcpy(cont + 0x58, nm, strlen(nm));
        cont[0x158] = 1;
    }
    char msg[192];
    sprintf_s(msg, "fake player state: %zu containers of 0x%zX at %p+0x%zX\n",
              kRun, kCont, (void*)psd, kFirst);
    OutputDebugStringA(msg);
    // Leaked on purpose, like the other fixtures.
}
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Live-layout inventories: the control for phase L.
//
// Built to the layout measured against the running game, not to a guess -- Nick's
// exosuit, starship and storage container 0 all read 10/H/?/count/?/ptr/count, and
// the elements matched his screen exactly. Three objects at IRREGULAR offsets,
// because that is how the real ones sit: one ~120 MB allocation, clustered within
// ~28 KB, with no stride between them.
//
// The usual caveat applies and is the reason it is written down: this fixture is
// built to the same layout phase L tests, so it proves the scan, the validation and
// the report, and proves nothing about the layout itself. The layout's evidence is
// the live game -- FUEL1 x2163 at grid (4,0) and SAND1 x820 at (6,1), both
// confirmed against the UI, and a Cheat Engine address that matched ours exactly.
//
// One object is deliberately malformed: counts that disagree between +0x08 and
// +0x18. It must be rejected, because that disagreement is the cheap test doing
// nearly all the work in the real scan.
static void PublishFakeLiveInventories() {
    struct Seed { const char* id; int amount; int x, y; };
    // The awkward-but-real cases, which is the whole point of them being here. The
    // first live run validated ZERO inventories because Nick's suit holds an item
    // the UI shows as "inf 1,307" -- an unbounded stack whose MaxAmount is not >= 1
    // -- and one such slot vetoed all 32. A fixture where every stack is a tidy
    // x/9999 cannot catch that, and did not.
    static const Seed suit[] = {
        {"FUEL1", 2163, 4, 0}, {"SAND1", 820, 6, 1}, {"LAND1", 5319, 2, 0},
        {"CATALYST1", 1430, 3, 0}, {"OXYGEN", 618, 6, 0},
        {"^JET1", 1, 0, 3},          // installed technology: '^' prefix
        {"TRA_MINERALS3", 1307, 1, 0},   // unbounded stack, MaxAmount 0
    };
    static const Seed ship[] = {
        {"FUEL1", 971, 3, 1}, {"ASTEROID1", 445, 0, 0},
    };
    static const Seed chest[] = {
        {"REACTION2", 4, 8, 0}, {"ASTEROID3", 982, 1, 0},
    };
    static const Seed spare[] = {
        {"YELLOW2", 77, 0, 0},
    };

    // WHAT THIS FIXTURE CAN AND CANNOT PROVE. It is written from the layout ReNMS
    // describes, so it CANNOT confirm that layout -- only the live game can. What
    // it does prove is that the probe reads and reports the fields it claims to,
    // and one thing more, which is the point:
    //
    //   the tail between size and capacity is filled with garbage that is NOT
    //   element-shaped, so a probe that walks capacity REJECTS every inventory
    //   here and a probe that walks size accepts them.
    //
    // That is a control that fails on the old code, which is the only kind worth
    // having. It is also exactly what the live game did to us: 85 declared where
    // 33 were real.
    struct Build {
        const Seed* seeds; int n;
        int w, h;
        int miCapacity;     // +0x04, and the popcount the mask is given
        int size, cap;      // mStore.size and mStore.capacity
        int lattice;        // index in the fixed array, or -1 for off-lattice
    };
    static const Build builds[] = {
        {suit,  7, 10, 12, 30, 10, 15,  0},
        {ship,  2, 10,  5, 40,  5, 12,  1},
        {chest, 2, 10,  6, 50,  4,  9,  2},
        {spare, 1,  3,  3,  4,  2,  6,  5},   // a gap at 3 and 4: members missing
        {spare, 1,  7,  5, 12,  1,  4, -1},   // off the lattice entirely
    };
    const int kBuilds = (int)(sizeof(builds) / sizeof(builds[0]));

    // A fixed array of stores at a constant stride, most of them empty, which is
    // what cTkFixedArray<cGcInventoryStore, N> looks like when only some ships are
    // owned. The stride is arbitrary here -- sizeof(cGcInventoryStore) is not known
    // -- so the control proves the lattice detector copes with MISSING members, not
    // that any particular stride is right.
    const size_t kStride  = 0x1F0;
    const size_t kLattice = 0x400;
    const size_t kOffLat  = 0x3040;      // not congruent to kLattice mod kStride

    unsigned char* pool = (unsigned char*)calloc(1, 0x8000);
    if (!pool) return;
    for (int b = 0; b < kBuilds; ++b) {
        const Build& bd = builds[b];
        size_t at = bd.lattice >= 0 ? kLattice + (size_t)bd.lattice * kStride : kOffLat;
        unsigned char* obj = pool + at;

        unsigned char* arr = (unsigned char*)calloc((size_t)bd.cap, 0x30);
        if (!arr) return;
        for (int k = 0; k < bd.n; ++k) {
            unsigned char* e = arr + (size_t)k * 0x30;
            const Seed& sd = bd.seeds[k];
            memcpy(e + 0x00, sd.id, strlen(sd.id));
            // MaxAmount 0 for the unbounded stack, and 1 for installed technology.
            int mx = 9999;
            if (strcmp(sd.id, "TRA_MINERALS3") == 0) mx = 0;
            if (sd.id[0] == '^') mx = 1;
            memcpy(e + 0x10, &sd.x, 4);
            memcpy(e + 0x14, &sd.y, 4);
            memcpy(e + 0x18, &sd.amount, 4);
            memcpy(e + 0x20, &mx, 4);
        }
        // [n, size) stay zeroed -- properly empty slots, which are real and must
        // not be mistaken for malformed ones.
        // [size, cap) is the stale allocation. Bytes that cannot be an id.
        for (int k = bd.size; k < bd.cap; ++k)
            memset(arr + (size_t)k * 0x30, 0x01, 0x30);

        unsigned short w = (unsigned short)bd.w, h = (unsigned short)bd.h;
        short micap = (short)bd.miCapacity;
        unsigned long long p64 = (unsigned long long)(void*)arr;
        unsigned int cap = (unsigned int)bd.cap, size = (unsigned int)bd.size;
        memcpy(obj + 0x00, &w, 2);
        memcpy(obj + 0x02, &h, 2);
        memcpy(obj + 0x04, &micap, 2);
        memcpy(obj + 0x08, &cap, 4);
        memcpy(obj + 0x0C, &size, 4);
        memcpy(obj + 0x10, &p64, 8);
        memcpy(obj + 0x18, &cap, 4);      // history capacity: the gate compares these
        memcpy(obj + 0x1C, &size, 4);
        memcpy(obj + 0x20, &p64, 8);

        // mxValidSlots at -0x80: miCapacity bits, low 16 of each word only.
        int left = bd.miCapacity;
        for (int word = 0; word < 16 && left > 0; ++word) {
            int take = left < 16 ? left : 16;
            unsigned long long bits = (take == 16) ? 0xFFFFull
                                                   : ((1ull << take) - 1ull);
            memcpy(obj - 0x80 + (size_t)word * 8, &bits, 8);
            left -= take;
        }
    }
    // The one that must be rejected: the two capacities disagree.
    {
        unsigned char* obj = pool + 0x7000;
        unsigned char* arr = (unsigned char*)calloc(8, 0x30);
        if (arr) {
            memcpy(arr, "FUEL1", 5);
            int amt = 1, mx = 9999;
            memcpy(arr + 0x18, &amt, 4);
            memcpy(arr + 0x20, &mx, 4);
            unsigned long long p64 = (unsigned long long)(void*)arr;
            unsigned int a = 8, bcount = 9, size = 8;
            memcpy(obj + 0x08, &a, 4);
            memcpy(obj + 0x0C, &size, 4);
            memcpy(obj + 0x10, &p64, 8);
            memcpy(obj + 0x18, &bcount, 4);
        }
    }
    char msg[160];
    sprintf_s(msg, "fake live inventories: %d valid (4 on a 0x%zX lattice) + 1 malformed,"
                   " pool at %p\n", kBuilds, kStride, (void*)pool);
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
    PublishFakeInventory();
    PublishFakeIdPool();
    PublishFakePlayerState();
    PublishFakeLiveInventories();
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

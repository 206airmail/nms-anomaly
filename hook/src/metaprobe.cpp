// Probe: is the game's own type metadata walkable in memory?
//
// WHY THIS FILE EXISTS
// --------------------
// Every other hook in this DLL sits at the OS boundary and knows nothing about
// No Man's Sky -- which is exactly why they survive patches. This file is the
// deliberate exception, and it is a *measurement*, not an API. It answers one
// question and writes down the evidence either way:
//
//     does NMS carry a runtime table that names its own classes and members?
//
// If it does, a read/write API for game state can be generated from that table
// per build instead of maintained by hand, and the whole shape of that project
// changes. If it does not, we find that out in one session instead of three
// months.
//
// THE REASONING BEHIND THE METHOD
// -------------------------------
// The game has to be able to read an .MBIN, and an .MBIN names its template
// ("cGcPlayerStateData") rather than describing it. So the field layout of every
// serialisable class must exist somewhere in the process. If those descriptors
// are records in a table, each record near-certainly holds a pointer to the
// class-name string. Hence:
//
//   pass 1    find the class-name strings in NMS.exe's read-only data
//   pass 2    find every aligned 8-byte value in the process equal to one of
//             those string addresses -- each hit is a candidate descriptor
//   analysis  if hits for *different* classes sit at a constant stride, that
//             stride is the record size and we have found the table
//   analysis  follow the record's other pointers one level down: an array whose
//             records each begin with a pointer to a short identifier is a
//             member table, i.e. field names. That is the outright win.
//
// A name we guess wrongly costs nothing: it is reported as "not found" and the
// rest of the pass is unaffected. That is why the list is long, why it is
// overridable from probe_names.txt without rebuilding, and why it is not worth
// agonising over. Being wrong here is cheap; being narrow is not.
//
// WHERE the table turns out to live matters as much as whether it exists:
//   - in an image section  -> static, build-constant offsets, dump once per patch
//   - in private memory    -> built at runtime, so a reader needs a pointer chase
//                             from a static root rather than a fixed offset
// The report says which, because the two imply different designs.
//
// SAFETY
// ------
// Reading arbitrary addresses in a live game is the entire risk here. Two
// independent layers, because either one alone is not enough:
//
//   1. every address is checked against a VirtualQuery map before it is touched;
//   2. every loop that touches game memory is SEH-guarded, because that map goes
//      stale the moment the game frees something, and it is a live game.
//
// The probe thread sets t_inHook, which both stops our own file hooks observing
// the report being written and stops the vectored handler in crash.cpp logging
// the faults the probe provokes on purpose.
//
// Functions containing __try here touch PODs only -- no C++ object in the same
// frame needs unwinding -- which is a hard MSVC requirement, not a style choice.
// Results come back through caller-provided fixed arrays for the same reason.
#include "common.h"
#define PSAPI_VERSION 2   // K32GetProcessMemoryInfo from kernel32, no psapi.lib
#include <psapi.h>
#include <vector>
#include <string>
#include <utility>
#include <algorithm>
#include <cstdio>
#include <cstdarg>
#include <cstring>

namespace {

// ---------------------------------------------------------------- tunables --
constexpr int    kMaxNames          = 512;
constexpr int    kMaxStrHitsPerName = 8;       // the same name can be interned twice
constexpr int    kMaxHits           = 40000;   // total pass-2 hits we keep
constexpr int    kMaxHitsPerRegion  = 8192;
constexpr size_t kPrivateRegionSkip = 512ull << 20;   // streaming pools, not descriptors
constexpr size_t kThrottleEvery     = 64ull << 20;    // yield to the game this often
constexpr int    kIdentMax          = 96;
constexpr int    kRecordDumpMax     = 256;     // bytes of a record we hex-dump
constexpr int    kRecordsToDump     = 6;
constexpr int    kGroupsToDump      = 3;
// A descriptor record holds a name pointer *and something else*, so a smaller
// gap than this is not a record size. It also keeps the name-offset consistency
// test meaningful: at a stride of 8 every offset is 0 and the test says nothing.
constexpr size_t kMinStride         = 0x10;

// Class names worth looking for. Two kinds deliberately mixed: the metadata
// system's own probable class names (finding one of those is direct proof) and
// ordinary data classes -- including the inventory ones, because those are what
// a state-reading API would want first.
const char* const kDefaultNames[] = {
    // the reflection machinery itself
    "cTkMetaDataClass", "cTkMetaDataMember", "cTkMetaDataFunctionLookup",
    "cTkMetaDataXMLFunctions", "cTkMetaData", "TkMetaDataClass", "TkMetaDataMember",
    // inventory and player state -- the first things a real API would touch
    "cGcPlayerStateData", "cGcInventoryContainer", "cGcInventoryLayout",
    "cGcInventoryElement", "cGcInventoryTable", "cGcInventoryIndex",
    "cGcInventoryStore", "cGcInventoryTechProperties",
    // broad, well-known data classes, for corroboration and stride inference
    "cGcRealitySubstanceData", "cGcRealityManagerData", "cGcDebugOptions",
    "cGcSolarSystemData", "cGcSpaceshipComponentData", "cGcTechnologyTable",
    "cGcGameplayGlobals", "cGcSceneSettings", "cGcAISpaceshipManagerData",
    "cTkModelResource", "cTkSceneNodeData", "cTkAttachmentData",
    "cGcResourceElement", "cGcProductData", "cGcProductTable",
    "cGcRewardTableData", "cGcFreighterBaseTable",
};

// ------------------------------------------------------------ build identity --
// Any finding below is true of exactly one build. Record which one, so that a
// repeat after the next patch is a comparison rather than a fresh guess.
struct ImageInfo {
    uintptr_t base = 0;
    size_t    size = 0;
    DWORD     timeStamp = 0;
    DWORD     checkSum = 0;
};
ImageInfo g_img;

bool ReadImageInfo() {
    HMODULE h = GetModuleHandleW(nullptr);
    if (!h) return false;
    auto* dos = (const IMAGE_DOS_HEADER*)h;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return false;
    auto* nt = (const IMAGE_NT_HEADERS64*)((const char*)h + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE) return false;
    g_img.base      = (uintptr_t)h;
    g_img.size      = nt->OptionalHeader.SizeOfImage;
    g_img.timeStamp = nt->FileHeader.TimeDateStamp;
    g_img.checkSum  = nt->OptionalHeader.CheckSum;
    return true;
}

std::string SectionOf(uintptr_t a) {
    if (!g_img.base || a < g_img.base || a >= g_img.base + g_img.size) return {};
    auto* dos = (const IMAGE_DOS_HEADER*)g_img.base;
    auto* nt  = (const IMAGE_NT_HEADERS64*)((const char*)g_img.base + dos->e_lfanew);
    auto* sec = IMAGE_FIRST_SECTION(nt);
    for (unsigned i = 0; i < nt->FileHeader.NumberOfSections; ++i, ++sec) {
        uintptr_t s = g_img.base + sec->VirtualAddress;
        if (a >= s && a < s + sec->Misc.VirtualSize) {
            char n[9] = {0};
            memcpy(n, sec->Name, 8);
            return n;
        }
    }
    return "?";
}

// ---------------------------------------------------------------- region map --
struct Region {
    uintptr_t base;
    size_t    size;
    DWORD     type;        // MEM_IMAGE / MEM_PRIVATE / MEM_MAPPED
    DWORD     protect;
};
std::vector<Region> g_regions;

bool ProtectReadable(DWORD p) {
    if (p & (PAGE_GUARD | PAGE_NOACCESS)) return false;
    switch (p & 0xFF) {
        case PAGE_READONLY: case PAGE_READWRITE: case PAGE_WRITECOPY:
        case PAGE_EXECUTE_READ: case PAGE_EXECUTE_READWRITE: case PAGE_EXECUTE_WRITECOPY:
            return true;
        default:
            return false;
    }
}

void BuildRegionMap() {
    g_regions.clear();
    MEMORY_BASIC_INFORMATION mbi;
    uintptr_t a = 0;
    while (VirtualQuery((LPCVOID)a, &mbi, sizeof(mbi)) == sizeof(mbi)) {
        uintptr_t base = (uintptr_t)mbi.BaseAddress;
        size_t    sz   = mbi.RegionSize;
        if (mbi.State == MEM_COMMIT && ProtectReadable(mbi.Protect))
            g_regions.push_back(Region{base, sz, mbi.Type, mbi.Protect});
        uintptr_t next = base + sz;
        if (next <= a) break;              // wrapped or stuck: stop rather than spin
        a = next;
    }
}

const Region* FindRegion(uintptr_t a) {
    size_t lo = 0, hi = g_regions.size();
    while (lo < hi) {                      // last region whose base <= a
        size_t mid = (lo + hi) / 2;
        if (g_regions[mid].base <= a) lo = mid + 1; else hi = mid;
    }
    if (lo == 0) return nullptr;
    const Region& r = g_regions[lo - 1];
    return (a >= r.base && a < r.base + r.size) ? &r : nullptr;
}

// Deliberately conservative: a struct straddling two adjacent regions reads as
// unreadable. A false "no" here costs a missed annotation; a false "yes" costs
// the player's session.
bool Readable(uintptr_t a, size_t n) {
    if (!a || a + n < a) return false;
    const Region* r = FindRegion(a);
    return r && (a + n) <= (r->base + r->size);
}

// Address ranges belonging to the probe itself. The target array is a run of
// pointers to class names, so it is indistinguishable from a real descriptor
// table unless it is named and dropped; the probe thread stack is the same story
// for anything spilled onto it. Checked per hit rather than per qword -- the scan
// loop is bandwidth-bound and must not grow a branch.
std::vector<std::pair<uintptr_t, uintptr_t>> g_exclude;

void Exclude(uintptr_t lo, uintptr_t hi) {
    if (hi > lo) g_exclude.push_back(std::make_pair(lo, hi));
}

bool Excluded(uintptr_t a) {
    for (auto& e : g_exclude) if (a >= e.first && a < e.second) return true;
    return false;
}

// ------------------------------------------------- SEH leaves (PODs only) ----

// Finds `name` as a whole NUL-terminated token. Requiring the terminator and a
// non-identifier byte in front is what stops "cGcInventoryContainer" matching
// inside "cGcInventoryContainerTable" and inflating every count downstream.
int FindStringExact(const char* name, size_t len, uintptr_t base, size_t size,
                    uintptr_t* out, int outMax, int* faulted) {
    int n = 0;
    *faulted = 0;
    if (size < len + 2 || outMax <= 0) return 0;
    const char* p    = (const char*)base;
    const char* stop = (const char*)(base + size - (len + 1));
    __try {
        for (; p < stop; ++p) {
            if (*p != name[0]) continue;
            if (memcmp(p, name, len) != 0) continue;
            if (p[len] != '\0') continue;
            char b = (p > (const char*)base) ? p[-1] : '\0';
            if ((b >= 'A' && b <= 'Z') || (b >= 'a' && b <= 'z') ||
                (b >= '0' && b <= '9') || b == '_') continue;
            out[n++] = (uintptr_t)p;
            if (n >= outMax) break;
            p += len;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    return n;
}

// One pass over a region collecting every aligned qword equal to one of the
// sorted target addresses. The range reject in front matters more than it looks:
// all the class-name strings live in one narrow span of .rdata, so almost every
// qword in the process is rejected by a single compare and the scan ends up
// limited by memory bandwidth rather than by how many names we search for.
//
// It records the target's INDEX, never the address itself. That is not a
// micro-optimisation: a buffer holding target addresses is itself a run of
// qwords pointing at class names, so the probe finds its own scratch space and
// reports it as a beautifully regular table. Measured, the first time this ran:
// 8192 hits at a gap of 8 (the old value array) and 362 at a gap of 0x20
// (sizeof(Hit) in the results vector) -- both of which outranked the real table.
int ScanForTargets(const uintptr_t* targets, int nTargets,
                   uintptr_t base, size_t size,
                   uintptr_t* hitAt, int* hitIdx, int outMax,
                   int* faulted, size_t* scanned) {
    int n = 0;
    *faulted = 0;
    *scanned = 0;
    if (nTargets <= 0 || outMax <= 0 || size < 8) return 0;
    uintptr_t lowT = targets[0], highT = targets[nTargets - 1];
    const uintptr_t* p   = (const uintptr_t*)base;
    const uintptr_t* end = (const uintptr_t*)(base + (size & ~(size_t)7));
    __try {
        for (; p < end; ++p) {
            uintptr_t v = *p;
            if (v < lowT || v > highT) continue;
            int lo = 0, hi = nTargets - 1;
            while (lo <= hi) {
                int mid = (lo + hi) >> 1;
                if (targets[mid] == v) {
                    hitAt[n]  = (uintptr_t)p;
                    hitIdx[n] = mid;
                    ++n;
                    break;
                }
                if (targets[mid] < v) lo = mid + 1; else hi = mid - 1;
            }
            if (n >= outMax) break;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    *scanned = (size_t)((const char*)p - (const char*)base);
    return n;
}

int ReadIdentifierRaw(uintptr_t a, char* out, int cap) {
    int n = 0;
    __try {
        const char* p = (const char*)a;
        for (; n < cap - 1; ++n) {
            char c = p[n];
            if (c == '\0') break;
            bool ok = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
                      (c >= '0' && c <= '9') || c == '_';
            if (!ok) return 0;
            out[n] = c;
        }
        if (p[n] != '\0') return 0;         // ran out of room: not a short identifier
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return 0;
    }
    out[n] = '\0';
    return n;
}

bool SafeRead(uintptr_t src, void* dst, size_t n) {
    __try {
        memcpy(dst, (const void*)src, n);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// ------------------------------------------------------------- C++ helpers ---

std::string Fmt(const char* fmt, ...) {
    char b[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(b, sizeof(b), fmt, ap);
    va_end(ap);
    return b;
}

// The identifier at `a`, or empty. Clamped to the containing region so the SEH
// guard is the second line of defence rather than the first.
std::string ReadIdentifier(uintptr_t a) {
    const Region* r = FindRegion(a);
    if (!r) return {};
    size_t avail = (r->base + r->size) - a;
    int cap = (int)(avail < (size_t)kIdentMax ? avail : (size_t)kIdentMax);
    if (cap < 3) return {};
    char buf[kIdentMax];
    int n = ReadIdentifierRaw(a, buf, cap);
    if (n < 2) return {};
    char c0 = buf[0];
    if (!((c0 >= 'A' && c0 <= 'Z') || (c0 >= 'a' && c0 <= 'z') || c0 == '_')) return {};
    return std::string(buf, n);
}

std::string RegionKind(const Region& r) {
    const char* t = r.type == MEM_IMAGE   ? "image"
                  : r.type == MEM_PRIVATE ? "private"
                  : r.type == MEM_MAPPED  ? "mapped" : "?";
    std::string s = t;
    std::string sec = SectionOf(r.base);
    if (!sec.empty()) s += " " + sec;
    return s;
}

// What one qword inside a candidate record looks like. This annotation is the
// point of the whole dump: reading `-> "Health"` or `int 42` beside an offset is
// how a record layout gets identified by eye.
std::string Annotate(uintptr_t v) {
    if (v == 0) return "null";
    if (v < 0x10000) return Fmt("int %llu", (unsigned long long)v);
    if (!Readable(v, 1)) {
        // Two small 32-bit fields packed into one qword -- most likely a size
        // beside a member count, which is exactly the shape worth spotting.
        // Calling it "not readable" implies we tried to follow a pointer.
        unsigned lo = (unsigned)(v & 0xFFFFFFFFull), hi = (unsigned)(v >> 32);
        if (lo < 0x1000000u && hi < 0x1000000u)
            return Fmt("u32 pair 0x%X, 0x%X", lo, hi);
        return "not a mapped address";
    }
    std::string id = ReadIdentifier(v);
    if (!id.empty()) return Fmt("-> \"%s\"", id.c_str());
    const Region* r = FindRegion(v);
    std::string k = r ? RegionKind(*r) : std::string("?");
    if (Readable(v, 8)) {
        uintptr_t inner = 0;
        if (SafeRead(v, &inner, 8)) {
            std::string id2 = Readable(inner, 1) ? ReadIdentifier(inner) : std::string();
            if (!id2.empty())
                return Fmt("-> [%s] -> \"%s\"   <= array of named records?", k.c_str(), id2.c_str());
            return Fmt("-> [%s] first qword 0x%llX", k.c_str(), (unsigned long long)inner);
        }
    }
    return Fmt("-> [%s]", k.c_str());
}

// Does `arr` look like an array of member descriptors -- records that each begin
// (at some small fixed offset) with a pointer to a short identifier? If so we
// have field names, which is the difference between "there is a table somewhere"
// and "the type system is walkable".
bool LooksLikeMemberTable(uintptr_t arr, size_t* outStride, size_t* outNameOff,
                          std::vector<std::string>* names) {
    static const size_t kStrides[]  = {0x08, 0x10, 0x18, 0x20, 0x28, 0x30,
                                       0x38, 0x40, 0x48, 0x50, 0x60};
    static const size_t kNameOffs[] = {0x00, 0x08};
    for (size_t no : kNameOffs) {
        for (size_t s : kStrides) {
            if (s <= no) continue;
            std::vector<std::string> got;
            for (int i = 0; i < 12; ++i) {
                uintptr_t slot = arr + (size_t)i * s + no;
                if (!Readable(slot, 8)) break;
                uintptr_t p = 0;
                if (!SafeRead(slot, &p, 8)) break;
                std::string id = Readable(p, 1) ? ReadIdentifier(p) : std::string();
                if (id.empty()) break;
                got.push_back(id);
            }
            // Four consecutive named records is far past coincidence, and short
            // enough that a class with only a handful of members still trips it.
            if (got.size() >= 4) {
                *outStride  = s;
                *outNameOff = no;
                *names      = got;
                return true;
            }
        }
    }
    return false;
}

// -------------------------------------------------------------- name list ----

std::vector<std::string> ReadNameFile() {
    std::vector<std::string> out;
    std::wstring path = g_outDir + L"probe_names.txt";
    HANDLE h = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return out;
    std::string text;
    char buf[8192];
    DWORD got = 0;
    while (ReadFile(h, buf, sizeof(buf), &got, nullptr) && got) text.append(buf, got);
    CloseHandle(h);
    size_t i = 0;
    while (i < text.size()) {
        size_t e = text.find_first_of("\r\n", i);
        if (e == std::string::npos) e = text.size();
        std::string line = text.substr(i, e - i);
        i = e + 1;
        size_t a = line.find_first_not_of(" \t");
        if (a == std::string::npos) continue;
        size_t b = line.find_last_not_of(" \t");
        line = line.substr(a, b - a + 1);
        if (line.empty() || line[0] == '#') continue;
        out.push_back(line);
    }
    return out;
}

// The leading 'c' is a convention, not a guarantee -- the same class turns up as
// both cGcFoo and GcFoo depending on where it is written down -- so search both
// spellings and let the miss be free.
std::vector<std::string> BuildNameList(bool* fromFile) {
    std::vector<std::string> names = ReadNameFile();
    *fromFile = !names.empty();
    if (names.empty())
        for (const char* n : kDefaultNames) names.push_back(n);

    std::vector<std::string> all;
    auto add = [&all](const std::string& s) {
        if (s.size() < 3 || (int)all.size() >= kMaxNames) return;
        for (auto& e : all) if (e == s) return;
        all.push_back(s);
    };
    for (auto& n : names) {
        add(n);
        if (n.size() > 2 && n[0] == 'c' && n[1] >= 'A' && n[1] <= 'Z') add(n.substr(1));
        else if (n.size() > 2 && n[0] >= 'A' && n[0] <= 'Z') add("c" + n);
    }
    return all;
}

// ------------------------------------------------------------------ the run --

// No field here holds the class-name address, only the index of its name. Store
// the address and this vector becomes a run of pointers to class names at a
// constant stride of sizeof(Hit) -- which is to say, the probe manufactures a
// textbook descriptor table and then discovers it. Measured: 36 phantom hits at
// a gap of 0x20, ranked above the real table.
struct Hit {
    uintptr_t at;          // address of the pointer we found
    int       nameIdx;     // which class name it pointed at
    size_t    regionIdx;
};

struct Group {
    size_t              regionIdx = 0;
    std::vector<size_t> hits;            // indices into the hit list, ascending by .at
    size_t              stride = 0;
    int                 strideCount = 0;
    int                 distinctNames = 0;
    std::vector<size_t> nameOffsets;     // name-pointer offset within the record
};

void AppendReport(std::string& rep, const std::string& s) {
    constexpr size_t kRepMax = 8ull << 20;
    if (rep.size() < kRepMax) rep += s;
}

void WriteReportFile(const std::wstring& path, const std::string& text) {
    HANDLE h = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        logger::Pushf(Level::Warn, "metaprobe", "could not write %s (error %lu)",
                      ToUtf8(path).c_str(), GetLastError());
        return;
    }
    const char* p = text.data();
    size_t left = text.size();
    while (left) {
        DWORD chunk = (DWORD)(left > (1u << 20) ? (1u << 20) : left), done = 0;
        if (!WriteFile(h, p, chunk, &done, nullptr) || !done) break;
        p += done; left -= done;
    }
    CloseHandle(h);
}

void RunProbe(int runNo) {
    ULONGLONG t0 = GetTickCount64();
    std::string rep;
    std::wstring outPath = g_outDir + L"metaprobe_" + std::to_wstring(runNo) + L".txt";

    ReadImageInfo();
    BuildRegionMap();

    size_t totalCommitted = 0, imageBytes = 0, privateBytes = 0;
    for (auto& r : g_regions) {
        totalCommitted += r.size;
        if (r.type == MEM_IMAGE) imageBytes += r.size;
        else if (r.type == MEM_PRIVATE) privateBytes += r.size;
    }

    AppendReport(rep, Fmt(
        "NMS metadata probe -- run %d, hook " NMSLOG_HOOK_VERSION "\r\n"
        "================================================================\r\n"
        "One question: does the game carry a runtime table naming its own classes\r\n"
        "and their members? The verdict is at the bottom; everything between here\r\n"
        "and it is the evidence for that verdict.\r\n\r\n"
        "build:   NMS.exe base 0x%llX  size 0x%llX  timestamp 0x%08lX  checksum 0x%08lX\r\n"
        "memory:  %llu readable regions, %.1f MB committed "
        "(image %.1f MB, private %.1f MB)\r\n",
        runNo, (unsigned long long)g_img.base, (unsigned long long)g_img.size,
        g_img.timeStamp, g_img.checkSum,
        (unsigned long long)g_regions.size(), totalCommitted / 1048576.0,
        imageBytes / 1048576.0, privateBytes / 1048576.0));

    bool fromFile = false;
    std::vector<std::string> names = BuildNameList(&fromFile);
    AppendReport(rep, Fmt("names:   %llu searched, from %s\r\n\r\n",
                          (unsigned long long)names.size(),
                          fromFile ? "probe_names.txt" : "the built-in list"));

    // ---- pass 1: the class-name strings, in the main image only -------------
    //
    // Cost here is names x image bytes, and NMS.exe is a 118 MB image of which 52
    // MB is .text. A NUL-terminated class name is not kept in code, so executable
    // regions are skipped by default and the pass roughly halves. MetaProbeScanCode
    // puts them back, which is the thing to try first if this finds nothing.
    ULONGLONG t1 = GetTickCount64();
    AppendReport(rep, "PASS 1 -- class-name strings in NMS.exe\r\n"
                      "----------------------------------------------------------------\r\n");
    std::vector<uintptr_t> strAddr;
    std::vector<int>       strName;
    strAddr.reserve(names.size());
    strName.reserve(names.size());
    int namesFound = 0;
    size_t p1Scanned = 0, p1SkippedCode = 0;
    size_t p1Yield = 0;
    for (size_t ni = 0; ni < names.size(); ++ni) {
        uintptr_t found[kMaxStrHitsPerName];
        int n = 0;
        for (auto& r : g_regions) {
            if (n >= kMaxStrHitsPerName) break;
            if (r.type != MEM_IMAGE) continue;
            if (r.base < g_img.base || r.base >= g_img.base + g_img.size) continue;
            bool exec = (r.protect & 0xFF) == PAGE_EXECUTE_READ ||
                        (r.protect & 0xFF) == PAGE_EXECUTE_READWRITE ||
                        (r.protect & 0xFF) == PAGE_EXECUTE_WRITECOPY;
            if (exec && !g_config.metaProbeScanCode) {
                if (ni == 0) p1SkippedCode += r.size;   // count the section once, not per name
                continue;
            }
            int faulted = 0;
            n += FindStringExact(names[ni].c_str(), names[ni].size(), r.base, r.size,
                                 found + n, kMaxStrHitsPerName - n, &faulted);
            p1Scanned += r.size;
            p1Yield   += r.size;
            // A long name list makes this the expensive pass, and run 2 happens
            // while the game is being played. Give the scheduler a gap.
            if (p1Yield >= kThrottleEvery) { p1Yield = 0; Sleep(1); }
        }
        if (!n) continue;
        ++namesFound;
        std::string line = Fmt("  %-34s %d:", names[ni].c_str(), n);
        for (int k = 0; k < n; ++k) {
            strAddr.push_back(found[k]);
            strName.push_back((int)ni);
            line += Fmt("  NMS.exe+0x%llX(%s)",
                        (unsigned long long)(found[k] - g_img.base),
                        SectionOf(found[k]).c_str());
        }
        AppendReport(rep, line + "\r\n");
    }
    for (size_t ni = 0; ni < names.size(); ++ni) {
        bool hit = false;
        for (int idx : strName) if (idx == (int)ni) { hit = true; break; }
        if (!hit) AppendReport(rep, Fmt("  %-34s not found\r\n", names[ni].c_str()));
    }
    AppendReport(rep, Fmt(
        "\r\n  %d of %llu names present as whole tokens.\r\n"
        "  scanned %.1f MB over all names in %llu ms; skipped %.1f MB of executable\r\n"
        "  sections (set MetaProbeScanCode=1 to include them)\r\n\r\n",
        namesFound, (unsigned long long)names.size(),
        p1Scanned / 1048576.0, (unsigned long long)(GetTickCount64() - t1),
        p1SkippedCode / 1048576.0));

    if (strAddr.empty()) {
        AppendReport(rep,
            "VERDICT: NOT FOUND\r\n"
            "----------------------------------------------------------------\r\n"
            "  None of the searched names exist as whole strings in this image.\r\n"
            "  Either the name list is wrong -- likeliest, and fixed by writing a\r\n"
            "  probe_names.txt generated from the real mod corpus -- or the class\r\n"
            "  names are not kept as plain text in the binary at all.\r\n");
        WriteReportFile(outPath, rep);
        logger::Pushf(Level::Warn, "metaprobe",
                      "verdict NOT FOUND: none of %llu class names exist as strings in NMS.exe "
                      "| detail in %s",
                      (unsigned long long)names.size(), ToUtf8(outPath).c_str());
        g_regions.clear();
        g_regions.shrink_to_fit();
        return;
    }

    // Sort the string addresses together with their name index: the scan needs a
    // sorted target array, and every hit has to map back to a name afterwards.
    {
        std::vector<size_t> order(strAddr.size());
        for (size_t i = 0; i < order.size(); ++i) order[i] = i;
        std::sort(order.begin(), order.end(),
                  [&](size_t a, size_t b) { return strAddr[a] < strAddr[b]; });
        std::vector<uintptr_t> sa;
        std::vector<int>       sn;
        sa.reserve(order.size());
        sn.reserve(order.size());
        for (size_t i : order) { sa.push_back(strAddr[i]); sn.push_back(strName[i]); }
        strAddr.swap(sa);
        strName.swap(sn);
    }

    // ---- pass 2: pointers to those strings ---------------------------------
    AppendReport(rep, "PASS 2 -- aligned qwords pointing at those strings\r\n"
                      "----------------------------------------------------------------\r\n");
    std::vector<Hit> hits;
    g_exclude.clear();
    Exclude((uintptr_t)strAddr.data(), (uintptr_t)(strAddr.data() + strAddr.size()));
    // The probe thread stack carries the scratch arrays below, and scanning it
    // says nothing about the game.
    ULONG_PTR stackLo = 0, stackHi = 0;
    GetCurrentThreadStackLimits(&stackLo, &stackHi);
    Exclude((uintptr_t)stackLo, (uintptr_t)stackHi);

    size_t scannedBytes = 0, skippedBig = 0, skippedMapped = 0, faultedRegions = 0;
    size_t cappedRegions = 0, excludedHits = 0;
    size_t budget = (size_t)g_config.metaProbeHeapBudgetMB << 20;
    size_t sinceYield = 0;
    bool   truncated = false;

    // Image regions first: they are small, they cannot move, and a table found
    // there is the good outcome. Only then the heap, and only under a budget.
    for (int phase = 0; phase < 2 && !truncated; ++phase) {
        if (phase == 1 && !g_config.metaProbeScanHeap) break;
        for (size_t ri = 0; ri < g_regions.size(); ++ri) {
            const Region& r = g_regions[ri];
            if (phase == 0 && r.type != MEM_IMAGE) continue;
            if (phase == 1) {
                if (r.type == MEM_MAPPED) { skippedMapped += r.size; continue; }
                if (r.type != MEM_PRIVATE) continue;
                if (r.size > kPrivateRegionSkip) { skippedBig += r.size; continue; }
                if (scannedBytes >= budget) { truncated = true; break; }
            }
            // Skip our own stack outright: it is where the scratch arrays live,
            // and it would otherwise fill the hit cap with the probe's own data.
            if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
            uintptr_t at[kMaxHitsPerRegion];
            int       idx[kMaxHitsPerRegion];
            int faulted = 0;
            size_t did = 0;
            int n = ScanForTargets(strAddr.data(), (int)strAddr.size(), r.base, r.size,
                                   at, idx, kMaxHitsPerRegion, &faulted, &did);
            scannedBytes += did;
            sinceYield   += did;
            if (faulted) ++faultedRegions;
            if (n >= kMaxHitsPerRegion) ++cappedRegions;
            for (int k = 0; k < n && (int)hits.size() < kMaxHits; ++k) {
                if (Excluded(at[k])) { ++excludedHits; continue; }
                hits.push_back(Hit{at[k], strName[idx[k]], ri});
            }
            if ((int)hits.size() >= kMaxHits) { truncated = true; break; }
            // A game is running on the other threads. Give the scheduler a gap
            // rather than holding a core flat for the whole heap pass.
            if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
        }
    }

    AppendReport(rep, Fmt(
        "  scanned %.1f MB, %llu hits%s\r\n"
        "  skipped %.1f MB in regions over %llu MB, and %.1f MB file-mapped\r\n"
        "  %llu regions faulted mid-scan (freed under us; expected in a live game)\r\n"
        "  %llu hits discarded as the probe's own memory; %llu regions hit the "
        "per-region cap of %d\r\n\r\n",
        scannedBytes / 1048576.0, (unsigned long long)hits.size(),
        truncated ? "  (TRUNCATED -- raise MetaProbeHeapBudgetMB to see the rest)" : "",
        skippedBig / 1048576.0, (unsigned long long)(kPrivateRegionSkip >> 20),
        skippedMapped / 1048576.0, (unsigned long long)faultedRegions,
        (unsigned long long)excludedHits, (unsigned long long)cappedRegions,
        kMaxHitsPerRegion));

    // ---- analysis: is there a stride? --------------------------------------
    AppendReport(rep, Fmt(
        "ANALYSIS -- do the hits form a table?\r\n"
        "----------------------------------------------------------------\r\n"
        "  A group is the hits within one memory region. Groups are ranked by\r\n"
        "  coherence, not by size: one consistent name offset at a stride of at\r\n"
        "  least 0x%llX first, then distinct classes, then hit count.\r\n"
        "  Small phantom groups are normal. Freed heap still holds whatever was\r\n"
        "  written there, including copies of this probe's own list of target\r\n"
        "  addresses, which look like a table with an 8-byte stride.\r\n",
        (unsigned long long)kMinStride));

    std::vector<Group> groups;
    {
        std::vector<size_t> order(hits.size());
        for (size_t i = 0; i < order.size(); ++i) order[i] = i;
        std::sort(order.begin(), order.end(), [&](size_t a, size_t b) {
            if (hits[a].regionIdx != hits[b].regionIdx)
                return hits[a].regionIdx < hits[b].regionIdx;
            return hits[a].at < hits[b].at;
        });
        for (size_t i = 0; i < order.size();) {
            Group g;
            g.regionIdx = hits[order[i]].regionIdx;
            while (i < order.size() && hits[order[i]].regionIdx == g.regionIdx)
                g.hits.push_back(order[i++]);
            groups.push_back(std::move(g));
        }
    }

    for (auto& g : groups) {
        // The commonest gap between consecutive hits is the record size, if there
        // is one at all.
        std::vector<std::pair<size_t, int>> hist;
        for (size_t i = 1; i < g.hits.size(); ++i) {
            uintptr_t d = hits[g.hits[i]].at - hits[g.hits[i - 1]].at;
            if (d < kMinStride || d > 4096 || (d & 7)) continue;
            bool found = false;
            for (auto& e : hist) if (e.first == (size_t)d) { ++e.second; found = true; break; }
            if (!found) hist.push_back(std::make_pair((size_t)d, 1));
        }
        std::sort(hist.begin(), hist.end(),
                  [](const std::pair<size_t, int>& a, const std::pair<size_t, int>& b) {
                      return a.second > b.second;
                  });
        if (!hist.empty()) { g.stride = hist[0].first; g.strideCount = hist[0].second; }

        std::vector<int> seen;
        for (size_t hi : g.hits) {
            int ni = hits[hi].nameIdx;
            bool dup = false;
            for (int s : seen) if (s == ni) { dup = true; break; }
            if (!dup) seen.push_back(ni);
        }
        g.distinctNames = (int)seen.size();

        // Where the name pointer sits inside each record, measured from the
        // lowest hit. In a real table that is one number; a spread of values
        // means the records are not records, which is the most reliable way to
        // tell a table from a coincidence.
        if (g.stride) {
            uintptr_t L = hits[g.hits[0]].at;
            for (size_t hi : g.hits) {
                size_t off = (size_t)((hits[hi].at - L) % g.stride);
                bool dup = false;
                for (size_t o : g.nameOffsets) if (o == off) { dup = true; break; }
                if (!dup) g.nameOffsets.push_back(off);
            }
        }
    }

    // Rank by how much each group looks like a table of many different classes.
    // A single consistent name offset comes first, ahead of sheer hit count: a
    // big noisy region will always win on volume, and never on coherence.
    std::sort(groups.begin(), groups.end(), [](const Group& a, const Group& b) {
        bool ca = a.nameOffsets.size() == 1 && a.stride >= kMinStride;
        bool cb = b.nameOffsets.size() == 1 && b.stride >= kMinStride;
        if (ca != cb) return ca;
        if (a.distinctNames != b.distinctNames) return a.distinctNames > b.distinctNames;
        return a.strideCount > b.strideCount;
    });

    for (size_t gi = 0; gi < groups.size() && gi < 12; ++gi) {
        const Group& g = groups[gi];
        const Region& r = g_regions[g.regionIdx];
        AppendReport(rep, Fmt(
            "  group %llu: %s at 0x%llX -- %llu hits, %d distinct classes, "
            "commonest gap 0x%llX x%d, %llu distinct name offsets\r\n",
            (unsigned long long)gi, RegionKind(r).c_str(), (unsigned long long)r.base,
            (unsigned long long)g.hits.size(), g.distinctNames,
            (unsigned long long)g.stride, g.strideCount,
            (unsigned long long)g.nameOffsets.size()));
    }
    AppendReport(rep, "\r\n");

    // ---- the records themselves -------------------------------------------
    bool memberTableFound = false;
    std::string memberEvidence;

    for (size_t gi = 0; gi < groups.size() && gi < (size_t)kGroupsToDump; ++gi) {
        const Group& g = groups[gi];
        if (g.stride == 0 || g.strideCount < 2) continue;
        const Region& r = g_regions[g.regionIdx];
        uintptr_t L = hits[g.hits[0]].at;
        size_t s = g.stride;

        AppendReport(rep, Fmt(
            "RECORDS -- group %llu, %s, assuming a record size of 0x%llX\r\n"
            "----------------------------------------------------------------\r\n"
            "  Record boundaries are taken from the lowest hit (0x%llX), so the\r\n"
            "  name-pointer offset below is relative to that. One offset repeated\r\n"
            "  across every record means the layout is real.\r\n",
            (unsigned long long)gi, RegionKind(r).c_str(),
            (unsigned long long)s, (unsigned long long)L));

        const std::vector<size_t>& offs = g.nameOffsets;
        std::string offList;
        for (size_t o : offs) offList += Fmt(" 0x%llX", (unsigned long long)o);
        AppendReport(rep, Fmt(
            "  name pointer at record offset:%s  (%llu distinct%s)\r\n\r\n",
            offList.c_str(), (unsigned long long)offs.size(),
            offs.size() == 1 ? " -- consistent"
                             : " -- inconsistent, so the stride is probably wrong"));

        int dumped = 0;
        for (size_t hi : g.hits) {
            if (dumped >= kRecordsToDump) break;
            uintptr_t h = hits[hi].at;
            uintptr_t recStart = L + ((h - L) / s) * s;
            size_t len = s < (size_t)kRecordDumpMax ? s : (size_t)kRecordDumpMax;
            if (!Readable(recStart, len)) continue;
            std::string cls = (hits[hi].nameIdx >= 0) ? names[hits[hi].nameIdx] : "?";
            AppendReport(rep, Fmt("  record for \"%s\" at 0x%llX:\r\n",
                                  cls.c_str(), (unsigned long long)recStart));
            for (size_t off = 0; off + 8 <= len; off += 8) {
                uintptr_t v = 0;
                if (!SafeRead(recStart + off, &v, 8)) break;
                AppendReport(rep, Fmt("    +0x%02llX  %016llX  %s\r\n",
                                      (unsigned long long)off, (unsigned long long)v,
                                      Annotate(v).c_str()));
                // The payoff: does this field point at a table of member names?
                if (v > 0x10000 && Readable(v, 8)) {
                    size_t ms = 0, mno = 0;
                    std::vector<std::string> mnames;
                    if (LooksLikeMemberTable(v, &ms, &mno, &mnames)) {
                        memberTableFound = true;
                        std::string list;
                        for (size_t k = 0; k < mnames.size(); ++k)
                            list += (k ? ", " : "") + mnames[k];
                        AppendReport(rep, Fmt(
                            "      ^^ MEMBER TABLE: record size 0x%llX, name at +0x%llX -- %s\r\n",
                            (unsigned long long)ms, (unsigned long long)mno, list.c_str()));
                        if (memberEvidence.empty())
                            memberEvidence = cls + " members: " + list;
                    }
                }
            }
            AppendReport(rep, "\r\n");
            ++dumped;
        }
    }

    // ---- verdict -----------------------------------------------------------
    const Group* best = groups.empty() ? nullptr : &groups[0];
    std::string verdict, detail, where;
    if (memberTableFound) {
        verdict = "WALKABLE";
        detail  = "class records carry a member table with field names (" + memberEvidence + ")";
    } else if (best && best->strideCount >= 6 && best->distinctNames >= 3 &&
               best->nameOffsets.size() == 1 && best->stride >= kMinStride) {
        verdict = "TABLE LIKELY";
        detail  = Fmt("%d distinct classes at a constant stride of 0x%llX in %s, but no "
                      "member table was recognised one level down",
                      best->distinctNames, (unsigned long long)best->stride,
                      RegionKind(g_regions[best->regionIdx]).c_str());
    } else if (!hits.empty()) {
        verdict = "BACK-REFS ONLY";
        detail  = Fmt("%llu pointers to class names exist but form no regular table -- "
                      "they may be nothing more than call sites",
                      (unsigned long long)hits.size());
    } else {
        verdict = "NAMES ONLY";
        detail  = "the class-name strings are in the binary, but nothing in the memory "
                  "we scanned points at them";
    }

    if (best && (verdict == "WALKABLE" || verdict == "TABLE LIKELY")) {
        where = (g_regions[best->regionIdx].type == MEM_IMAGE)
            ? "  The table is in an image section, so its offsets are constant for this\r\n"
              "  build: dump it once per patch and generate headers from the dump.\r\n"
            : "  The table is in private memory, so it is built at runtime: a reader needs\r\n"
              "  a pointer chase from a static root, not a fixed offset.\r\n";
    }

    AppendReport(rep, Fmt(
        "VERDICT: %s\r\n"
        "----------------------------------------------------------------\r\n"
        "  %s\r\n%s\r\n  probe took %llu ms\r\n",
        verdict.c_str(), detail.c_str(), where.c_str(),
        (unsigned long long)(GetTickCount64() - t0)));

    WriteReportFile(outPath, rep);

    // The summary goes through the normal log and pipe so the app sees it; the hex
    // stays in the file, where it is not going to swamp a session.
    logger::Pushf(verdict == "WALKABLE" || verdict == "TABLE LIKELY" ? Level::Info : Level::Warn,
                  "metaprobe",
                  "verdict %s: %s | %d/%llu names found, %llu hits, %.1f MB scanned in %llu ms "
                  "| detail in %s",
                  verdict.c_str(), detail.c_str(), namesFound,
                  (unsigned long long)names.size(), (unsigned long long)hits.size(),
                  scannedBytes / 1048576.0, (unsigned long long)(GetTickCount64() - t0),
                  ToUtf8(outPath).c_str());

    g_regions.clear();
    g_regions.shrink_to_fit();
    g_exclude.clear();
}

// ===========================================================================
// INSTANCE PROBE -- find live inventory containers in the running game.
//
// The metadata probe answers "what shape is a cGcInventoryContainer". This one
// answers "where is one right now", which is a different problem and the only
// one that needs a save loaded.
//
// It works because the layout is known, so the search is a signature match
// rather than a blind scan. cGcInventoryElement is the ideal anchor: its first
// field is a 16-byte NUL-padded ASCII item id ("CARBON", "^LAUNCHFUEL"), which
// is far too structured to occur by accident, and the integers beside it
// constrain each other -- Amount <= MaxAmount, DamageFactor in [0,1], and two
// fields that can only be 0 or 1.
//
// Three phases:
//   A  scan private memory for runs of valid elements at the array stride
//   B  one pass looking for a pointer to each run -- a dynamic array handle
//      sits at container+0x10, so a hit names a candidate container
//   C  validate the container's own fields and print what is in it
//
// EVERY OFFSET BELOW IS MEASURED, AND ONLY FOR ONE BUILD. They come from the
// descriptor table in NMS.exe (timestamp 0x6AB0FFC9), extracted by
// tools/nms_meta_extract.py, where 2731 of 2734 classes tile without overlap.
// After a game patch, re-extract before trusting a single one of them.
// ===========================================================================

// cGcInventoryElement -- tiled size 0x2A, but 0x30 as an array element, which is
// the `size` the container's dynamic-array member declares.
// ---- cGcPlayerStateData holds every inventory INLINE, back to back ----------
//
// Read out of the offline extraction (tools/nms_meta_extract.py), not guessed:
// cGcPlayerStateData's 27 members of type cGcInventoryContainer occupy
// 0x809D8 .. 0x82EF8 with no gap at all -- 27 x 0x160 = 0x2520 exactly. The
// container is 0x159 bytes and 0x160 is that aligned up.
//
// Why this matters more than any leaf signature. Four attempts to find an
// inventory by recognising one container or one element failed, because a single
// record's fields are weak evidence: mostly-zero records satisfy any test built
// from bounds, and the save editor's own notes say Width and Height LIE (a sold
// ship reads 1x1, machinery 16x1) -- which is exactly the field
// ScanForContainerCandidates filters on first, and it rejects the width 0 that
// every unowned chest has.
//
// A run of 27 containers at a fixed stride is a different kind of evidence: it
// is periodic structure, it cannot arise by coincidence, and it identifies the
// owning object rather than one leaf. Once one run is found, all 263 members of
// cGcPlayerStateData are at known offsets from it.
constexpr size_t kContStride        = 0x160;    // container size aligned up
constexpr size_t kPsdContainers     = 27;       // inline containers in the run
constexpr size_t kPsdFirstContainer = 0x809D8;  // offset of the run within the class
constexpr size_t kPsdRunBytes       = kPsdContainers * kContStride;

// MEASURED, and it changes the target. NOTHING in the 2,741 classes owns a member
// of type cGcPlayerStateData -- it is a root, and the save editor reaches it at the
// JSON path BaseContext.PlayerStateData. So the 0x86A11 (551 KB) struct with 27
// inline containers is the SAVE DOCUMENT, materialised when the game saves or
// loads, which is why 7.6 GB of mid-game memory contained no trace of it.
//
// Two much smaller classes each hold three inline containers, contiguous, and
// those are per-entity objects that should exist while playing:
//
//   cGcPlayerOwnershipData  0x530  Inventory @0x20, _Cargo @0x180, _TechOnly @0x2E0
//   cGcFreighterSaveData    0x4F9  Inventory @0x30, _Cargo @0x190, _TechOnly @0x2F0
//
// So a run of 3 is as interesting as a run of 27, and it is the one to expect
// mid-game. A run of 3 is weak evidence on its own, which is exactly why the
// members must be content-validated rather than merely periodic.
constexpr int kMinRunExtent = 3;
constexpr size_t kOwnershipFirstContainer = 0x20;
constexpr size_t kFreighterFirstContainer = 0x30;

// In offset order, which is the order they appear in memory -- NOT declaration
// order and not alphabetical by accident: Chest10 really does precede Chest1.
const char* const kPsdContainerNames[kPsdContainers] = {
    "Chest10Inventory", "Chest1Inventory", "Chest2Inventory", "Chest3Inventory",
    "Chest4Inventory", "Chest5Inventory", "Chest6Inventory", "Chest7Inventory",
    "Chest8Inventory", "Chest9Inventory", "ChestMagic2Inventory",
    "ChestMagicInventory", "CookingIngredientsInventory",
    "CorvetteStorageInventory", "FishBaitBoxInventory", "FishPlatformInventory",
    "FoodUnitInventory", "FreighterInventory", "FreighterInventory_Cargo",
    "FreighterInventory_TechOnly", "GraveInventory", "Inventory",
    "Inventory_Cargo", "Inventory_TechOnly", "RocketLockerInventory",
    "ShipInventory", "WeaponInventory",
};

constexpr size_t kElemStride    = 0x30;
constexpr size_t kElemId        = 0x00;   // string16, NUL-padded
constexpr size_t kElemIndexX    = 0x10;   // cGcInventoryIndex { X, Y } -- 8 bytes, not 12
constexpr size_t kElemIndexY    = 0x14;
constexpr size_t kElemAmount    = 0x18;
constexpr size_t kElemDamage    = 0x1C;
constexpr size_t kElemMaxAmount = 0x20;
constexpr size_t kElemType      = 0x24;   // enum
constexpr size_t kElemAdded     = 0x28;
constexpr size_t kElemInstalled = 0x29;

// cGcInventoryContainer -- 0x159 bytes
constexpr size_t kContSlots   = 0x10;     // dynamic-array handle -> elements
// A dynamic array occupies 0x10 bytes inline, and a pointer only accounts for 8 of
// them. Reading the next dword as the element count is a hypothesis, so it is
// sanity-checked and reported rather than trusted.
constexpr size_t kContSlotCount = 0x18;
constexpr size_t kContClass   = 0x40;
constexpr size_t kContHeight  = 0x44;
constexpr size_t kContVersion = 0x50;
constexpr size_t kContWidth   = 0x54;
constexpr size_t kContName    = 0x58;     // string256
constexpr size_t kContIsCool  = 0x158;
constexpr size_t kContBaseStats   = 0x00;   // dynarray cGcInventoryBaseStatEntry
constexpr size_t kContSpecial     = 0x20;   // dynarray cGcInventorySpecialSlot
constexpr size_t kContValidIdx    = 0x30;   // dynarray cGcInventoryIndex
constexpr size_t kContFromTech    = 0x48;
constexpr size_t kContStackGroup = 0x4C;
constexpr size_t kContSize     = 0x159;

constexpr int    kMinRun       = 4;       // consecutive elements to call it an array
constexpr int    kMinNonEmpty  = 2;       // of which this many must hold an item
constexpr int    kMaxStack     = 10000000;
constexpr int    kMaxRuns      = 4096;
constexpr int    kMaxCandsPerRegion = 65536;

struct ElemView {
    char  id[17];
    int   x, y;
    int   amount, maxAmount;
    unsigned int  type;
    unsigned char added, installed;
    float damage;
    bool  empty;
};

bool IdChar(unsigned char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') ||
           c == '_' || c == '^' || c == '.' || c == '-' || c == '#';
}

// A slot is either empty (the id field is all zero) or holds a properly
// NUL-padded id. Requiring the padding to be zero is most of this test's power:
// random bytes that start like an id almost never end like one.
bool ReadElem(uintptr_t p, ElemView* out) {
    unsigned char b[kElemStride];
    if (!Readable(p, kElemStride) || !SafeRead(p, b, kElemStride)) return false;

    int n = 0;
    while (n < 16 && b[kElemId + n] != 0) ++n;
    bool allZero = (n == 0);
    if (allZero) {
        for (int i = 0; i < 16; ++i) if (b[kElemId + i] != 0) { allZero = false; break; }
    }
    if (!allZero) {
        if (n < 2 || n > 15) return false;                       // 15 so padding exists
        for (int i = 0; i < n; ++i) if (!IdChar(b[kElemId + i])) return false;
        for (int i = n; i < 16; ++i) if (b[kElemId + i] != 0) return false;
    }

    int amount, maxAmount, x, y;
    unsigned int type;
    float damage;
    memcpy(&x,         b + kElemIndexX,    4);
    memcpy(&y,         b + kElemIndexY,    4);
    memcpy(&amount,    b + kElemAmount,    4);
    memcpy(&maxAmount, b + kElemMaxAmount, 4);
    memcpy(&type,      b + kElemType,      4);
    memcpy(&damage,    b + kElemDamage,    4);

    if (amount < 0 || amount > kMaxStack) return false;
    if (maxAmount < 0 || maxAmount > kMaxStack) return false;
    if (b[kElemAdded] > 1 || b[kElemInstalled] > 1) return false;
    if (!(damage >= 0.0f && damage <= 1.0f)) return false;        // also rejects NaN
    if (x < -1 || x > 4096 || y < -1 || y > 4096) return false;
    if (!allZero && (maxAmount < 1 || amount > maxAmount)) return false;

    memset(out, 0, sizeof(*out));
    if (!allZero) memcpy(out->id, b + kElemId, (size_t)n);
    out->x = x; out->y = y;
    out->amount = amount; out->maxAmount = maxAmount;
    out->type = type;
    out->added = b[kElemAdded]; out->installed = b[kElemInstalled];
    out->damage = damage;
    out->empty = allZero;
    return true;
}

// Cheap pre-filter, inline and SEH-guarded, so the expensive validation only runs
// on positions that could possibly be an element. Without this the scan does a
// guarded memcpy every 8 bytes across gigabytes and never finishes.
int ScanForElemCandidates(uintptr_t base, size_t size, uintptr_t* out, int outMax,
                          int* faulted, size_t* scanned) {
    int n = 0;
    *faulted = 0;
    *scanned = 0;
    if (outMax <= 0 || size < kElemStride) return 0;
    const unsigned char* p = (const unsigned char*)base;
    const unsigned char* end = (const unsigned char*)(base + size - kElemStride);
    __try {
        for (; p < end; p += 8) {
            unsigned char c0 = p[0];
            if (!((c0 >= 'A' && c0 <= 'Z') || c0 == '^')) continue;   // ids are upper-case
            unsigned char c1 = p[1];
            if (!((c1 >= 'A' && c1 <= 'Z') || (c1 >= '0' && c1 <= '9') || c1 == '_')) continue;
            if (p[15] != 0) continue;                                 // NUL padding
            unsigned int amt, mx;
            memcpy(&amt, p + kElemAmount, 4);
            memcpy(&mx, p + kElemMaxAmount, 4);
            if (amt > (unsigned)kMaxStack || mx > (unsigned)kMaxStack) continue;
            out[n++] = (uintptr_t)p;
            if (n >= outMax) break;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    *scanned = (size_t)((const unsigned char*)p - (const unsigned char*)base);
    return n;
}

struct Run {
    uintptr_t base;
    int       count;
    int       nonEmpty;
};

// Grow an anchor into the whole array. Extending backwards matters: the first
// slots of an inventory are often empty, so the anchor is rarely slot zero.
bool GrowRun(uintptr_t anchor, Run* out) {
    ElemView e;
    uintptr_t first = anchor, last = anchor;
    int count = 1, nonEmpty = 1;
    for (uintptr_t p = anchor - kElemStride; ; p -= kElemStride) {
        if (p > anchor) break;                       // wrapped
        if (!ReadElem(p, &e)) break;
        first = p;
        ++count;
        if (!e.empty) ++nonEmpty;
        if (count > 4096) break;
    }
    for (uintptr_t p = anchor + kElemStride; ; p += kElemStride) {
        if (!ReadElem(p, &e)) break;
        last = p;
        ++count;
        if (!e.empty) ++nonEmpty;
        if (count > 4096) break;
    }
    (void)last;
    if (count < kMinRun || nonEmpty < kMinNonEmpty) return false;
    out->base = first;
    out->count = count;
    out->nonEmpty = nonEmpty;
    return true;
}

// A string256 field is either printable text with NUL padding or all zero.
// Anything else means this is not a container, so the caller needs to tell
// "no name" apart from "not a name" -- hence the bool.
bool ContainerName(uintptr_t cont, std::string* out) {
    char buf[256];
    if (!Readable(cont + kContName, 256) || !SafeRead(cont + kContName, buf, 256)) return false;
    int n = 0;
    while (n < 256 && buf[n] != 0) ++n;
    for (int i = 0; i < n; ++i) {
        unsigned char c = (unsigned char)buf[i];
        if (c < 0x20 || c > 0x7E) return false;
    }
    for (int i = n; i < 256; ++i) if (buf[i] != 0) return false;   // must be NUL-padded
    out->assign(buf, (size_t)n);
    return true;
}


// ---------------------------------------------------------------------------
// Searching for the CONTAINER rather than the element.
//
// The first attempt anchored on cGcInventoryElement, because a 16-byte ASCII item
// id looked unmistakable. It was not. Against the real game it matched a table of
// mission identifiers -- PROC_PRODS, WORMHUNT, EGG_PODS -- which also begin with a
// 16-byte upper-case string followed by small integers. The validator was built
// out of *absences* (nothing over a bound, nothing non-zero where padding
// belongs), and a mostly-zero record satisfies every rule of that kind.
//
// A container is a far better anchor, because its signature is positive and
// composite: four dynamic-array handles at 0x00/0x10/0x20/0x30, each a pointer
// plus a count; then a row of small enums and integers; then 256 bytes that must
// be a NUL-padded string; then a bool. And the rows and columns have to be big
// enough to hold the slots the Slots handle claims -- a constraint no coincidence
// satisfies.
//
// It is also much cheaper. Requiring Width at +0x54 to be a sane grid dimension
// rejects almost everything in a single dword load, where the element scan had to
// read fifty bytes per candidate.
constexpr int kContReasonOk = 0;
const char* const kContReasons[] = {
    "ok", "unreadable", "width", "height", "version", "class/stackGroup",
    "isCool", "name not a string256", "slots handle", "grid too small for slots",
    "other handles",
};

bool HandleLooksLikeArray(const unsigned char* c, size_t off, uintptr_t* ptr,
                          unsigned int* count) {
    uintptr_t p = 0;
    unsigned int n = 0;
    memcpy(&p, c + off, 8);
    memcpy(&n, c + off + 8, 4);
    *ptr = p;
    *count = n;
    if (p == 0) return n == 0;                    // an empty array is legitimate
    if (n > 100000) return false;
    return Readable(p, 1);
}

int ValidateContainer(uintptr_t b, unsigned char* c, uintptr_t* slotsOut,
                      unsigned int* slotCountOut, std::string* nameOut) {
    if (!Readable(b, kContSize) || !SafeRead(b, c, kContSize)) return 1;
    int width = 0, height = 0, version = 0, fromTech = 0;
    unsigned int cls = 0, ssg = 0;
    memcpy(&width, c + kContWidth, 4);
    memcpy(&height, c + kContHeight, 4);
    memcpy(&version, c + kContVersion, 4);
    memcpy(&fromTech, c + kContFromTech, 4);
    memcpy(&cls, c + kContClass, 4);
    memcpy(&ssg, c + kContStackGroup, 4);
    if (width < 1 || width > 4096) return 2;
    if (height < 1 || height > 4096) return 3;
    if (version < 0 || version > 10000) return 4;
    if (cls > 255 || ssg > 255) return 5;
    if (c[kContIsCool] > 1) return 6;
    if (!ContainerName(b, nameOut)) return 7;

    uintptr_t slots = 0;
    unsigned int nslots = 0;
    if (!HandleLooksLikeArray(c, kContSlots, &slots, &nslots)) return 8;
    if (slots == 0 || nslots < 1 || nslots > 4096) return 8;
    // The grid has to be able to hold what the array claims. This is the test that
    // a coincidence fails.
    if ((long long)width * height < (long long)nslots) return 9;
    if (fromTech < 0 || fromTech > 4096) return 9;

    uintptr_t p2 = 0, p3 = 0, p4 = 0;
    unsigned int n2 = 0, n3 = 0, n4 = 0;
    if (!HandleLooksLikeArray(c, kContBaseStats, &p2, &n2) ||
        !HandleLooksLikeArray(c, kContSpecial, &p3, &n3) ||
        !HandleLooksLikeArray(c, kContValidIdx, &p4, &n4)) return 10;

    *slotsOut = slots;
    *slotCountOut = nslots;
    return kContReasonOk;
}

// Cheap pre-filter: one dword, and a grid dimension is a very narrow target.
int ScanForContainerCandidates(uintptr_t base, size_t size, uintptr_t* out, int outMax,
                               int* faulted, size_t* scanned) {
    int n = 0;
    *faulted = 0;
    *scanned = 0;
    if (outMax <= 0 || size < kContSize) return 0;
    const unsigned char* p = (const unsigned char*)base;
    const unsigned char* end = (const unsigned char*)(base + size - kContSize);
    __try {
        for (; p < end; p += 8) {
            unsigned int w, h, ver;
            memcpy(&w, p + kContWidth, 4);
            if (w - 1u >= 4096u) continue;               // 1..4096, unsigned trick
            memcpy(&h, p + kContHeight, 4);
            if (h - 1u >= 4096u) continue;
            if (p[kContIsCool] > 1) continue;
            memcpy(&ver, p + kContVersion, 4);
            if (ver > 10000u) continue;
            out[n++] = (uintptr_t)p;
            if (n >= outMax) break;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    *scanned = (size_t)((const unsigned char*)p - (const unsigned char*)base);
    return n;
}

// A dynamic-array handle: {pointer, count, capacity} in 0x10 bytes. `populated`
// distinguishes a handle that owns something from a well-formed empty one -- both
// are valid, and an unowned chest is legitimately empty, so emptiness is never a
// rejection on its own.
int HandleShape(const unsigned char* p, int* populated) {
    unsigned long long ptr;
    unsigned int count, cap;
    memcpy(&ptr, p, 8);
    memcpy(&count, p + 8, 4);
    memcpy(&cap, p + 12, 4);
    *populated = 0;
    if (ptr == 0) return (count == 0 && cap == 0) ? 1 : 0;
    if (ptr & 7) return 0;                                   // 8-aligned allocation
    if (ptr < 0x10000ull || ptr > 0x7FFFFFFFFFFFull) return 0;   // user-space only
    if (cap == 0 || cap > 65536u || count > cap) return 0;
    *populated = 1;
    return 1;
}

// Positions that look like a *player-visible* container. The predicate is the
// save editor's own documented rule -- a real container has a non-empty
// ValidSlotIndices -- rather than anything to do with Width or Height. Four
// well-formed array handles in a row at +0x00/+0x10/+0x20/+0x30 is the shape;
// two of them populated is the evidence.
//
// Counters, not rejections: every stage reports how many positions it dropped,
// because "554,182 candidates rejected, no reason given" is how attempt 2 wasted
// a whole session.
int ScanForContainerRuns(uintptr_t base, size_t size, uintptr_t* out, int outMax,
                         unsigned int* stage, int* faulted, size_t* scanned) {
    int n = 0;
    *faulted = 0;
    *scanned = 0;
    if (outMax <= 0 || size < kContSize) return 0;
    const unsigned char* p = (const unsigned char*)base;
    const unsigned char* end = (const unsigned char*)(base + size - kContSize);
    __try {
        for (; p < end; p += 8) {
            int popStats = 0, popSlots = 0, popSpecial = 0, popValid = 0;
            if (!HandleShape(p + kContBaseStats, &popStats)) continue;
            ++stage[0];
            if (!HandleShape(p + kContSlots, &popSlots)) continue;
            ++stage[1];
            if (!HandleShape(p + kContSpecial, &popSpecial)) continue;
            ++stage[2];
            if (!HandleShape(p + kContValidIdx, &popValid)) continue;
            ++stage[3];
            if (!popSlots || !popValid) continue;   // a container someone can see
            ++stage[4];
            out[n++] = (uintptr_t)p;
            if (n >= outMax) break;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    *scanned = (size_t)((const unsigned char*)p - (const unsigned char*)base);
    return n;
}

// Does this candidate actually hold an inventory, as opposed to merely having
// four integer pairs that pass a bounds test?
//
// Run 1 of phase P found 175,187 candidates, clustered them into 134,684 runs, and
// reported a "full run" whose members read width=447, class=1444630864 and names
// "3JNT" and "inger2JNT" -- skeleton joint data. Periodic structure is not enough
// on its own: in 7.6 GB there is enough of it to manufacture any period you look
// for. The only evidence that settles it is following the Slots pointer and finding
// an item on the other end.
//
// Deliberately NOT tested: Width, Height, Class, StackSizeGroup, Name. The first
// two are documented as unreliable, and the last three are what phase 1 rejected
// 533,288 of its 539,000 candidates on while finding nothing.
int ContainerReasons[8];

bool ContainerHoldsItems(uintptr_t a, int* firstId, char* idOut, int idCap) {
    uintptr_t slotsPtr = 0;
    unsigned int count = 0, cap = 0;
    if (!SafeRead(a + kContSlots, &slotsPtr, 8)) { ++ContainerReasons[0]; return false; }
    if (!SafeRead(a + kContSlotCount, &count, 4)) { ++ContainerReasons[0]; return false; }
    if (!SafeRead(a + kContSlots + 0x0C, &cap, 4)) { ++ContainerReasons[0]; return false; }
    if (!slotsPtr || count == 0) { ++ContainerReasons[1]; return false; }
    if (count > 2048 || cap < count || cap > 8192) { ++ContainerReasons[2]; return false; }
    if (!Readable(slotsPtr, (size_t)count * kElemStride)) { ++ContainerReasons[3]; return false; }
    unsigned char cool = 2;
    if (SafeRead(a + kContIsCool, &cool, 1) && cool > 1) { ++ContainerReasons[4]; return false; }
    // One real item is enough, and it has to be a real id: NUL-padded, and with a
    // stack size that is not absurd.
    for (unsigned int e = 0; e < count && e < 64; ++e) {
        ElemView ev;
        if (!ReadElem(slotsPtr + (uintptr_t)e * kElemStride, &ev)) break;
        if (ev.empty) continue;
        if (ev.amount < 0 || ev.maxAmount <= 0 || ev.amount > ev.maxAmount * 4) continue;
        if (idOut && idCap > 0) {
            int k = 0;
            for (; k < idCap - 1 && ev.id[k]; ++k) idOut[k] = ev.id[k];
            idOut[k] = '\0';
        }
        if (firstId) *firstId = (int)e;
        return true;
    }
    ++ContainerReasons[5];
    return false;
}

// Returns the number of containers reported.
int FindContainers(std::string& rep) {
    AppendReport(rep,
        "PHASE 1 -- containers by their own signature\r\n"
        "----------------------------------------------------------------\r\n");
    ULONG_PTR stackLo = 0, stackHi = 0;
    GetCurrentThreadStackLimits(&stackLo, &stackHi);

    size_t scanned = 0, filtered = 0, sinceYield = 0, faultedRegions = 0;
    size_t budget = (size_t)g_config.instProbeBudgetMB << 20;
    int reason[16] = {0};
    int found = 0;
    std::vector<uintptr_t> cand(65536);
    std::vector<uintptr_t> reported;

    for (size_t ri = 0; ri < g_regions.size(); ++ri) {
        const Region& r = g_regions[ri];
        if (r.type != MEM_PRIVATE) continue;
        if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
        if (scanned >= budget) break;
        int faulted = 0;
        size_t did = 0;
        int n = ScanForContainerCandidates(r.base, r.size, cand.data(), 65536,
                                           &faulted, &did);
        scanned += did;
        sinceYield += did;
        filtered += (size_t)n;
        if (faulted) ++faultedRegions;
        for (int k = 0; k < n; ++k) {
            unsigned char c[kContSize];
            uintptr_t slots = 0;
            unsigned int nslots = 0;
            std::string nm;
            int why = ValidateContainer(cand[k], c, &slots, &nslots, &nm);
            if (why < 16) ++reason[why];
            if (why != kContReasonOk) continue;
            // One container per Slots array: the game keeps working copies, and a
            // second object over the same array is the same inventory.
            bool dup = false;
            for (uintptr_t s : reported) if (s == slots) { dup = true; break; }
            if (dup) continue;
            reported.push_back(slots);
            ++found;

            int width = 0, height = 0, version = 0;
            unsigned int cls = 0, ssg = 0;
            memcpy(&width, c + kContWidth, 4);
            memcpy(&height, c + kContHeight, 4);
            memcpy(&version, c + kContVersion, 4);
            memcpy(&cls, c + kContClass, 4);
            memcpy(&ssg, c + kContStackGroup, 4);
            AppendReport(rep, Fmt(
                "\r\n  CONTAINER at 0x%llX\r\n"
                "    name=%s  width=%d height=%d version=%d class=%u stackGroup=%u isCool=%u\r\n"
                "    slots array 0x%llX -- %u slots declared\r\n",
                (unsigned long long)cand[k], nm.empty() ? "(no name)" : nm.c_str(),
                width, height, version, cls, ssg, c[kContIsCool],
                (unsigned long long)slots, nslots));
            int listed = 0, occupied = 0;
            for (unsigned int e = 0; e < nslots && e < 512; ++e) {
                ElemView ev;
                if (!ReadElem(slots + (size_t)e * kElemStride, &ev)) {
                    AppendReport(rep, Fmt("      (slot %u did not read as an element)\r\n", e));
                    break;
                }
                if (ev.empty) continue;
                ++occupied;
                if (listed < 60) {
                    AppendReport(rep, Fmt(
                        "      [%2d,%2d] %-18s %7d / %-7d  type=%u dmg=%.2f%s%s\r\n",
                        ev.x, ev.y, ev.id, ev.amount, ev.maxAmount, ev.type, ev.damage,
                        ev.installed ? " installed" : "", ev.added ? " auto" : ""));
                    ++listed;
                }
            }
            AppendReport(rep, Fmt("      (%d occupied of %u)\r\n", occupied, nslots));
        }
        if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
    }

    AppendReport(rep, Fmt(
        "\r\n  scanned %.1f MB, %llu positions passed the cheap filter,\r\n"
        "  %d containers reported, %llu regions faulted\r\n"
        "  why the rest were rejected:\r\n",
        scanned / 1048576.0, (unsigned long long)filtered, found,
        (unsigned long long)faultedRegions));
    for (int i = 1; i < 11; ++i)
        if (reason[i]) AppendReport(rep, Fmt("    %-26s %d\r\n", kContReasons[i], reason[i]));
    AppendReport(rep, "\r\n");
    return found;
}

// Find cGcPlayerStateData by the periodicity of its inline container run.
int FindPlayerStateData(std::string& rep) {
    AppendReport(rep,
        "PHASE P -- cGcPlayerStateData by its run of 27 inline containers\r\n"
        "----------------------------------------------------------------\r\n"
        "  Not a leaf signature. 27 containers at a 0x160 stride spanning 0x2520\r\n"
        "  bytes is periodic structure, and periodic structure does not happen by\r\n"
        "  accident. Width and Height are deliberately NOT tested: they lie.\r\n\r\n");

    ULONG_PTR stackLo = 0, stackHi = 0;
    GetCurrentThreadStackLimits(&stackLo, &stackHi);

    std::vector<uintptr_t> cand;
    cand.reserve(1 << 18);
    std::vector<uintptr_t> batch(1 << 15);
    unsigned int stage[8] = {0};
    size_t scanned = 0, sinceYield = 0, faultedRegions = 0;
    size_t budget = (size_t)g_config.instProbeBudgetMB << 20;

    g_exclude.clear();
    Exclude((uintptr_t)stackLo, (uintptr_t)stackHi);

    for (size_t ri = 0; ri < g_regions.size(); ++ri) {
        const Region& r = g_regions[ri];
        if (r.type != MEM_PRIVATE) continue;
        if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
        if (scanned >= budget) break;
        int faulted = 0;
        size_t did = 0;
        int n = ScanForContainerRuns(r.base, r.size, batch.data(), (int)batch.size(),
                                     stage, &faulted, &did);
        if (faulted) ++faultedRegions;
        for (int k = 0; k < n; ++k)
            if (!Excluded(batch[k])) cand.push_back(batch[k]);
        scanned += did;
        sinceYield += did;
        if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
    }

    AppendReport(rep, Fmt(
        "  scanned %.1f MB (%llu regions faulted)\r\n"
        "  shape filter: %u had a BaseStatValues handle, %u also Slots, %u also\r\n"
        "  SpecialSlots, %u also ValidSlotIndices, %u had both populated\r\n"
        "  -> %llu candidate containers\r\n\r\n",
        scanned / 1048576.0, (unsigned long long)faultedRegions,
        stage[0], stage[1], stage[2], stage[3], stage[4],
        (unsigned long long)cand.size()));

    if (cand.empty()) {
        AppendReport(rep, "  Nothing had four array handles in a row. Either the\r\n"
                          "  handle layout {ptr,count,capacity} is wrong, or no save is\r\n"
                          "  loaded.\r\n\r\n");
        return 0;
    }

    // Validate before clustering, not after. Clustering 175,187 shape-only candidates
    // is what produced 134,684 spurious runs: at that density every address has a
    // neighbour at every multiple of 0x160, so the period proves nothing.
    std::sort(cand.begin(), cand.end());
    cand.erase(std::unique(cand.begin(), cand.end()), cand.end());
    size_t shapeOnly = cand.size();
    memset(ContainerReasons, 0, sizeof(ContainerReasons));
    std::vector<uintptr_t> real;
    real.reserve(4096);
    for (size_t i = 0; i < cand.size(); ++i)
        if (ContainerHoldsItems(cand[i], nullptr, nullptr, 0)) real.push_back(cand[i]);
    AppendReport(rep, Fmt(
        "  content check: %llu of %llu candidates actually hold a readable item\r\n"
        "  (a container is only believed if its Slots pointer leads to one)\r\n"
        "    unreadable handle        %d\r\n"
        "    empty or null Slots      %d\r\n"
        "    count/capacity implausible %d\r\n"
        "    slot array not readable  %d\r\n"
        "    IsCool not 0 or 1        %d\r\n"
        "    no slot held a valid item %d\r\n\r\n",
        (unsigned long long)real.size(), (unsigned long long)shapeOnly,
        ContainerReasons[0], ContainerReasons[1], ContainerReasons[2],
        ContainerReasons[3], ContainerReasons[4], ContainerReasons[5]));
    cand.swap(real);
    if (cand.empty()) {
        AppendReport(rep,
            "  Not one candidate's Slots pointer led to an item. Combined with the\r\n"
            "  fact that nothing owns a cGcPlayerStateData member, the likely reading\r\n"
            "  is that these containers exist only while a save is being written or\r\n"
            "  read. Probe on a save instead of mid-game.\r\n\r\n");
        return 0;
    }

    struct Cluster { uintptr_t lo; int members; int span; };
    std::vector<Cluster> clusters;
    for (size_t i = 0; i < cand.size(); ++i) {
        int members = 1, span = 0;
        for (size_t j = i + 1; j < cand.size(); ++j) {
            uintptr_t d = cand[j] - cand[i];
            if (d >= kPsdRunBytes) break;   // 26 strides is the largest gap in a 27-run
            if (d % kContStride == 0) { ++members; span = (int)(d / kContStride); }
        }
        if (members >= kMinRunExtent) clusters.push_back(Cluster{cand[i], members, span});
    }
    std::sort(clusters.begin(), clusters.end(),
              [](const Cluster& a, const Cluster& b) { return a.members > b.members; });

    AppendReport(rep, Fmt("  %llu clusters of 2+ containers at the 0x160 stride\r\n",
                          (unsigned long long)clusters.size()));
    int bestMembers = clusters.empty() ? 0 : clusters[0].members;
    int bestExtent = clusters.empty() ? 0 : clusters[0].span + 1;
    for (size_t i = 0; i < clusters.size() && i < 8; ++i)
        AppendReport(rep, Fmt("    0x%llX  %d members within %d strides\r\n",
                              (unsigned long long)clusters[i].lo,
                              clusters[i].members, clusters[i].span + 1));
    AppendReport(rep, "\r\n");

    if (bestMembers < 3) return 0;

    // The best cluster, member by member, with what is actually in each. The
    // absolute index within the run is NOT assumed: the lowest member could be any
    // of the 27, so every member is reported with its stride offset from the lowest
    // and the contents are left to identify it. Guessing the alignment and printing
    // a confident wrong name is how earlier phases produced junk that read as data.
    const Cluster& c = clusters[0];
    AppendReport(rep, Fmt(
        "  best cluster at 0x%llX: %d populated containers spanning %d strides.\r\n"
        "  The SPAN is the finding, not the count -- an unowned storage chest has an\r\n"
        "  empty ValidSlotIndices and legitimately does not appear, so a full run of\r\n"
        "  27 will normally have fewer than 27 populated members.\r\n"
        "  Contents below; the run's absolute position is not assumed, so names are\r\n"
        "  not claimed yet -- an exosuit with known items in it pins which stride\r\n"
        "  index is which container.\r\n",
        (unsigned long long)c.lo, c.members, c.span + 1));
    for (int k = 0; k <= c.span; ++k) {
        uintptr_t a = c.lo + (uintptr_t)k * kContStride;
        if (!Readable(a, kContSize)) continue;
        uintptr_t slotsPtr = 0;
        unsigned int slotCount = 0, validCount = 0, w = 0, h = 0, cls = 0;
        if (!SafeRead(a + kContSlots, &slotsPtr, 8)) continue;
        SafeRead(a + kContSlotCount, &slotCount, 4);
        SafeRead(a + kContValidIdx + 8, &validCount, 4);
        SafeRead(a + kContWidth, &w, 4);
        SafeRead(a + kContHeight, &h, 4);
        SafeRead(a + kContClass, &cls, 4);
        char name[68] = {0};
        SafeRead(a + kContName, name, 64);
        for (int z = 0; z < 64; ++z)
            if (name[z] && (name[z] < 0x20 || (unsigned char)name[z] > 0x7E)) { name[z] = 0; break; }
        AppendReport(rep, Fmt(
            "\r\n   [+%2d] 0x%llX  slots=%u valid=%u  width=%u height=%u class=%u  name=%.40s\r\n",
            k, (unsigned long long)a, slotCount, validCount, w, h, cls, name));
        if (!slotsPtr || slotCount == 0 || slotCount > 4096) continue;
        int shownIds = 0;
        AppendReport(rep, "         ");
        for (unsigned int e = 0; e < slotCount && shownIds < 10; ++e) {
            ElemView ev;
            if (!ReadElem(slotsPtr + (uintptr_t)e * kElemStride, &ev)) break;
            if (ev.empty) continue;
            AppendReport(rep, Fmt("%s%s x%d", shownIds ? ", " : "", ev.id, ev.amount));
            ++shownIds;
        }
        AppendReport(rep, shownIds ? "\r\n" : "(no readable items)\r\n");
    }

    // Every alignment the cluster permits, with the cGcPlayerStateData base each
    // one implies. One of these is right; the contents above say which.
    AppendReport(rep, Fmt(
        "\r\n  If the lowest member is run index i, the class base is at\r\n"
        "  0x%llX - 0x%X - i*0x160:\r\n", (unsigned long long)c.lo,
        (unsigned)kPsdFirstContainer));
    for (int i = 0; i < (int)kPsdContainers && i + c.span < (int)kPsdContainers; ++i)
        AppendReport(rep, Fmt("    i=%-2d -> base 0x%llX   (lowest member would be %s)\r\n",
                              i, (unsigned long long)(c.lo - kPsdFirstContainer
                                                      - (uintptr_t)i * kContStride),
                              kPsdContainerNames[i]));
    AppendReport(rep, "\r\n");
    return bestExtent;
}

void RunInstanceProbe(int runNo) {
    ULONGLONG t0 = GetTickCount64();
    std::string rep;
    std::wstring outPath = g_outDir + L"instprobe_" + std::to_wstring(runNo) + L".txt";

    ReadImageInfo();
    BuildRegionMap();
    g_exclude.clear();
    ULONG_PTR stackLo = 0, stackHi = 0;
    GetCurrentThreadStackLimits(&stackLo, &stackHi);
    Exclude((uintptr_t)stackLo, (uintptr_t)stackHi);

    AppendReport(rep, Fmt(
        "NMS instance probe -- run %d, hook " NMSLOG_HOOK_VERSION "\r\n"
        "================================================================\r\n"
        "Finding live cGcInventoryContainer objects by signature. Offsets come\r\n"
        "from the descriptor table in NMS.exe and are valid for ONE build; the\r\n"
        "image this ran against is stamped below.\r\n\r\n"
        "build:   NMS.exe base 0x%llX  timestamp 0x%08lX\r\n",
        runNo, (unsigned long long)g_img.base, g_img.timeStamp));

    // Phase P first, and deliberately: it looks for the OWNER of the inventories
    // rather than for an inventory. Four attempts at leaf signatures failed, and
    // the run of 27 inline containers is the one piece of structure in this object
    // graph that cannot be produced by coincidence.
    int psd = FindPlayerStateData(rep);
    // Phase 1 only when phase P found nothing: it re-walks all of private memory,
    // and running both cost 17 minutes for one answer. Its per-reason counters are
    // still worth having when phase P comes up empty.
    int direct = psd ? 0 : FindContainers(rep);
    if (psd)
        AppendReport(rep, "PHASE 1 -- skipped: phase P already identified containers.\r\n"
                          "----------------------------------------------------------------\r\n\r\n");

    // A run of containers at the right stride outranks any count of separately
    // matched ones: 27 of them is cGcPlayerStateData itself, and that is the
    // difference between "an inventory exists" and "we can address all of them".
    if (psd >= 3) {
        // Named only where a run length identifies one class. Run 1 announced
        // "This is cGcPlayerStateData" over skeleton joint data because the claim
        // was attached to periodicity rather than to content.
        const char* how = psd >= (int)kPsdContainers
            ? "  27 strides of item-holding containers. That is cGcPlayerStateData,\r\n"
              "  and all 263 of its members are now at known offsets -- but note it is\r\n"
              "  the SAVE DOCUMENT, so it is only valid while a save is in flight.\r\n"
            : psd == 3
            ? "  Exactly 3 -- the shape of cGcPlayerOwnershipData (0x530) and\r\n"
              "  cGcFreighterSaveData (0x4F9), which each hold Inventory, _Cargo and\r\n"
              "  _TechOnly inline. Which one it is follows from the contents: the\r\n"
              "  freighter's and the exosuit's items differ.\r\n"
            : "  A run of a length no known class explains. Real containers -- every\r\n"
              "  member held a readable item -- but the owning class is unidentified,\r\n"
              "  so do not assume an offset from it.\r\n";
        AppendReport(rep, Fmt(
            "VERDICT: CONTAINER RUN FOUND (spans %d of 27 strides at 0x160)\r\n"
            "----------------------------------------------------------------\r\n"
            "%s"
            "  %d containers also matched their own signature independently.\r\n"
            "  probe took %llu ms\r\n",
            psd, how, direct, (unsigned long long)(GetTickCount64() - t0)));
        WriteReportFile(outPath, rep);
        logger::Pushf(Level::Info, "instprobe",
                      "verdict CONTAINER RUN FOUND: a run spanning %d of 27 strides at "
                      "0x160 (%d containers by signature) in %llu ms | detail in %s",
                      psd, direct, (unsigned long long)(GetTickCount64() - t0),
                      ToUtf8(outPath).c_str());
        g_regions.clear();
        g_regions.shrink_to_fit();
        g_exclude.clear();
        return;
    }

    if (direct > 0) {
        AppendReport(rep, Fmt(
            "VERDICT: INVENTORY FOUND\r\n"
            "----------------------------------------------------------------\r\n"
            "  %d containers found by their own signature; the element hunt below\r\n"
            "  was not needed and did not run.\r\n"
            "  probe took %llu ms\r\n",
            direct, (unsigned long long)(GetTickCount64() - t0)));
        WriteReportFile(outPath, rep);
        logger::Pushf(Level::Info, "instprobe",
                      "verdict INVENTORY FOUND: %d containers by container signature in "
                      "%llu ms | detail in %s",
                      direct, (unsigned long long)(GetTickCount64() - t0),
                      ToUtf8(outPath).c_str());
        g_regions.clear();
        g_regions.shrink_to_fit();
        g_exclude.clear();
        return;
    }
    AppendReport(rep,
        "  Nothing validated, so the element hunt below runs as a fallback and a\r\n"
        "  diagnostic. Note that it matches non-inventory classes too: mission id\r\n"
        "  tables look the same to it.\r\n\r\n");

    // ---- phase A: runs of inventory elements -------------------------------
    std::vector<Run> runs;
    size_t scanned = 0, filterHits = 0, faultedRegions = 0;
    size_t budget = (size_t)g_config.instProbeBudgetMB << 20;
    size_t sinceYield = 0;
    bool truncated = false;

    std::vector<uintptr_t> cand(kMaxCandsPerRegion);
    for (size_t ri = 0; ri < g_regions.size() && !truncated; ++ri) {
        const Region& r = g_regions[ri];
        if (r.type != MEM_PRIVATE) continue;                 // instances live on the heap
        if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
        if (scanned >= budget) { truncated = true; break; }
        int faulted = 0;
        size_t did = 0;
        int n = ScanForElemCandidates(r.base, r.size, cand.data(), kMaxCandsPerRegion,
                                      &faulted, &did);
        scanned += did;
        sinceYield += did;
        filterHits += (size_t)n;
        if (faulted) ++faultedRegions;
        for (int k = 0; k < n && (int)runs.size() < kMaxRuns; ++k) {
            ElemView e;
            if (!ReadElem(cand[k], &e) || e.empty) continue;
            Run run;
            if (!GrowRun(cand[k], &run)) continue;
            bool dup = false;
            for (auto& x : runs)
                if (run.base >= x.base &&
                    run.base < x.base + (uintptr_t)x.count * kElemStride) { dup = true; break; }
            if (!dup) runs.push_back(run);
        }
        if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
    }

    AppendReport(rep, Fmt(
        "\r\nPHASE A -- runs of inventory elements\r\n"
        "----------------------------------------------------------------\r\n"
        "  scanned %.1f MB of private memory%s\r\n"
        "  %llu positions passed the cheap filter, %llu grew into runs\r\n"
        "  %llu regions faulted mid-scan\r\n\r\n",
        scanned / 1048576.0,
        truncated ? "  (TRUNCATED -- raise InstProbeBudgetMB)" : "",
        (unsigned long long)filterHits, (unsigned long long)runs.size(),
        (unsigned long long)faultedRegions));

    if (runs.empty()) {
        AppendReport(rep,
            "VERDICT: NO INVENTORY FOUND\r\n"
            "----------------------------------------------------------------\r\n"
            "  No run of valid cGcInventoryElement records was found. Either no save\r\n"
            "  is loaded, the scan budget stopped short of the heap that holds them,\r\n"
            "  or the layout has moved since the extract this was built from.\r\n");
        WriteReportFile(outPath, rep);
        logger::Push(Level::Warn, "instprobe", "verdict NO INVENTORY FOUND: no element runs in "
                                               "scanned private memory");
        g_regions.clear();
        g_exclude.clear();
        return;
    }

    // What is IN each run, not just how big it is. The first version of this listed
    // 111 runs by address and size and printed the contents of only the eight that
    // phase C happened to select -- all of which were mission tables. That was read
    // as "no inventory exists anywhere", when in truth 103 runs had never been
    // looked inside. An address and a count cannot tell an inventory from a stats
    // table; the ids can.
    for (size_t i = 0; i < runs.size() && i < 160; ++i) {
        AppendReport(rep, Fmt("  run %3llu: 0x%llX  %d slots, %d occupied  ",
                              (unsigned long long)i, (unsigned long long)runs[i].base,
                              runs[i].count, runs[i].nonEmpty));
        int shownIds = 0;
        for (int k = 0; k < runs[i].count && shownIds < 8; ++k) {
            ElemView e;
            if (!ReadElem(runs[i].base + (size_t)k * kElemStride, &e)) break;
            if (e.empty) continue;
            AppendReport(rep, Fmt("%s%s x%d", shownIds ? ", " : "", e.id, e.amount));
            ++shownIds;
        }
        AppendReport(rep, "\r\n");
    }
    AppendReport(rep, "\r\n");

    // ---- phase B: who points at those runs? --------------------------------
    // Every element boundary in every run, not just the run bases: GrowRun extends
    // backwards and can overshoot the real array start into neighbouring heap, and
    // the container stores the true base. Searching for one address we guessed
    // would miss the container whenever the guess was long.
    std::vector<uintptr_t> targets;
    constexpr size_t kMaxTargets = 16384;
    {   // Reserved up front on purpose. A growing vector of addresses leaves a copy
        // of itself in freed heap at every reallocation, and a run of pointers into
        // the element array is exactly what phase B is looking for -- so the probe
        // finds its own discarded scratch and calls each copy a container.
        size_t want = 0;
        for (auto& r : runs) want += (size_t)r.count;
        targets.reserve(want < kMaxTargets ? want : kMaxTargets);
    }
    for (auto& r : runs)
        for (int k = 0; k < r.count && targets.size() < kMaxTargets; ++k)
            targets.push_back(r.base + (size_t)k * kElemStride);
    std::sort(targets.begin(), targets.end());
    targets.erase(std::unique(targets.begin(), targets.end()), targets.end());
    Exclude((uintptr_t)targets.data(), (uintptr_t)(targets.data() + targets.size()));

    std::vector<uintptr_t> handles;
    size_t scannedB = 0;
    sinceYield = 0;
    for (size_t ri = 0; ri < g_regions.size(); ++ri) {
        const Region& r = g_regions[ri];
        if (r.type == MEM_MAPPED) continue;
        if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
        if (r.size > kPrivateRegionSkip) continue;
        uintptr_t at[1024];
        int idx[1024];
        int faulted = 0;
        size_t did = 0;
        int n = ScanForTargets(targets.data(), (int)targets.size(), r.base, r.size,
                               at, idx, 1024, &faulted, &did);
        scannedB += did;
        sinceYield += did;
        for (int k = 0; k < n; ++k)
            if (!Excluded(at[k])) handles.push_back(at[k]);
        if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
    }

    AppendReport(rep, Fmt(
        "PHASE B -- pointers into those runs (a dynamic-array handle sits at\r\n"
        "container+0x%llX, so each hit names a candidate container)\r\n"
        "----------------------------------------------------------------\r\n"
        "  %llu element addresses searched for, %.1f MB scanned, %llu pointers found\r\n\r\n",
        (unsigned long long)kContSlots, (unsigned long long)targets.size(),
        scannedB / 1048576.0, (unsigned long long)handles.size()));

    // ---- phase C: what actually holds a pointer to an element array? -------
    //
    // The first version of this asserted a layout -- container+0x10 is the Slots
    // handle, so subtract 0x10 -- and threw away everything that failed. Against
    // the real game that rejected all 250 candidates and reported no reason, which
    // is the least useful possible outcome. The fake could not have caught it
    // either: the synthetic container was built to the same assumption, so it
    // tested the probe against itself rather than against No Man's Sky.
    //
    // So this no longer decides. It accounts for *why* each candidate fails, tries
    // the plausible bases rather than one, and dumps raw memory around the pointer
    // so the real layout can be read off the page the way the class descriptor was.
    int good = 0;

    struct Cand {
        uintptr_t at;         // address of the pointer
        uintptr_t target;     // what it points at
        int       runIdx;
        int       elemIdx;    // which slot within the run
        int       why;        // index into kReasons; 7 means it validated
    };
    std::vector<Cand> cands;
    for (uintptr_t h : handles) {
        Cand c{h, 0, -1, -1, 0};
        if (!Readable(h, 8) || !SafeRead(h, &c.target, 8)) continue;
        for (size_t i = 0; i < runs.size(); ++i) {
            const Run& r = runs[i];
            if (c.target >= r.base && c.target < r.base + (uintptr_t)r.count * kElemStride) {
                c.runIdx = (int)i;
                c.elemIdx = (int)((c.target - r.base) / kElemStride);
                break;
            }
        }
        if (c.runIdx >= 0) cands.push_back(c);
    }

    // A container's Slots pointer points at element zero. A pointer into the middle
    // of an array is more likely an iterator or a cursor, so the ones aimed at a
    // base -- and at the biggest arrays -- are examined first.
    std::sort(cands.begin(), cands.end(), [&](const Cand& a, const Cand& b) {
        bool a0 = a.elemIdx == 0, b0 = b.elemIdx == 0;
        if (a0 != b0) return a0;
        return runs[a.runIdx].count > runs[b.runIdx].count;
    });

    // Why does the original hypothesis fail? Count the first check each candidate
    // trips, rather than reporting a bare zero.
    const char* kReasons[] = {"unreadable at base", "width/height", "version",
                              "class/stackGroup", "isCool", "name not a string256",
                              "slots pointer mismatch", "passed"};
    int reasonCount[8] = {0};
    for (Cand& cd : cands) {
        uintptr_t cont = cd.at >= kContSlots ? cd.at - kContSlots : 0;
        int why = 7;
        unsigned char c[kContSize];
        int width = 0, height = 0, version = 0;
        unsigned int cls = 0, ssg = 0;
        uintptr_t slots = 0;
        std::string nm;
        if (!cont || !Readable(cont, kContSize) || !SafeRead(cont, c, kContSize)) {
            why = 0;
        } else {
            memcpy(&width, c + kContWidth, 4);
            memcpy(&height, c + kContHeight, 4);
            memcpy(&version, c + kContVersion, 4);
            memcpy(&cls, c + kContClass, 4);
            memcpy(&ssg, c + kContStackGroup, 4);
            memcpy(&slots, c + kContSlots, 8);
            if (width < 1 || width > 4096 || height < 1 || height > 4096) why = 1;
            else if (version < 0 || version > 10000) why = 2;
            else if (cls > 255 || ssg > 255) why = 3;
            else if (c[kContIsCool] > 1) why = 4;
            else if (!ContainerName(cont, &nm)) why = 5;
            else if (slots != cd.target) why = 6;
        }
        cd.why = why;
        ++reasonCount[why];
        if (why == 7) ++good;
    }

    AppendReport(rep, Fmt(
        "PHASE C -- what holds those pointers?\r\n"
        "----------------------------------------------------------------\r\n"
        "  %llu of %llu pointers aim inside a known run; %llu aim at a run's first\r\n"
        "  element, which is where a Slots handle would point.\r\n\r\n"
        "  Testing the original hypothesis (container = pointer address - 0x%llX),\r\n"
        "  first check each candidate fails:\r\n",
        (unsigned long long)cands.size(), (unsigned long long)handles.size(),
        (unsigned long long)std::count_if(cands.begin(), cands.end(),
                                          [](const Cand& c) { return c.elemIdx == 0; }),
        (unsigned long long)kContSlots));
    for (int i = 0; i < 8; ++i)
        if (reasonCount[i])
            AppendReport(rep, Fmt("    %-24s %d\r\n", kReasons[i], reasonCount[i]));
    AppendReport(rep, "\r\n");

    // Whatever validated, show what is in it -- this is the actual deliverable, and
    // the only part a caller of this probe would want.
    for (const Cand& cd : cands) {
        if (cd.why != 7) continue;
        uintptr_t cont = cd.at - kContSlots;
        unsigned char c[kContSize];
        if (!SafeRead(cont, c, kContSize)) continue;
        int width = 0, height = 0, version = 0;
        unsigned int cls = 0, ssg = 0, declared = 0;
        uintptr_t slots = 0;
        memcpy(&width, c + kContWidth, 4);
        memcpy(&height, c + kContHeight, 4);
        memcpy(&version, c + kContVersion, 4);
        memcpy(&cls, c + kContClass, 4);
        memcpy(&ssg, c + kContStackGroup, 4);
        memcpy(&declared, c + kContSlotCount, 4);
        memcpy(&slots, c + kContSlots, 8);
        const Run& run = runs[cd.runIdx];
        int slotCount = (declared >= 1 && declared <= 4096) ? (int)declared : run.count;
        std::string nm;
        ContainerName(cont, &nm);
        AppendReport(rep, Fmt(
            "  CONTAINER at 0x%llX\r\n"
            "    name=%s  width=%d height=%d version=%d class=%u stackGroup=%u isCool=%u\r\n"
            "    slots array 0x%llX -- %d slots declared (handle+8 said %u; our scan\r\n"
            "    reached %d elements from 0x%llX)\r\n",
            (unsigned long long)cont, nm.empty() ? "(no name)" : nm.c_str(),
            width, height, version, cls, ssg, c[kContIsCool],
            (unsigned long long)slots, slotCount, declared,
            run.count, (unsigned long long)run.base));
        int listed = 0;
        for (int k = 0; k < slotCount && listed < 60; ++k) {
            ElemView e;
            if (!ReadElem(slots + (size_t)k * kElemStride, &e)) break;
            if (e.empty) continue;
            AppendReport(rep, Fmt(
                "      [%2d,%2d] %-18s %7d / %-7d  type=%u dmg=%.2f%s%s\r\n",
                e.x, e.y, e.id, e.amount, e.maxAmount, e.type, e.damage,
                e.installed ? " installed" : "", e.added ? " auto" : ""));
            ++listed;
        }
        AppendReport(rep, "\r\n");
    }

    // Try the bases that a 0x10-byte array handle could imply, instead of one.
    static const long kBases[] = {0x00, -0x08, -0x10, -0x18, -0x20, -0x28, -0x30};
    AppendReport(rep,
        "  Same candidates, every plausible base. A column of plausible width,\r\n"
        "  height and version at one offset is the layout; all-nonsense means the\r\n"
        "  pointer is not held by a container at all.\r\n\r\n");

    int shown = 0;
    for (const Cand& cd : cands) {
        if (shown >= 8) break;
        if (cd.why == 7) continue;          // it worked; its contents are above
        ++shown;
        AppendReport(rep, Fmt(
            "  --- candidate %d: pointer at 0x%llX -> run %d element %d "
            "(run has %d slots, %d occupied)\r\n",
            shown, (unsigned long long)cd.at, cd.runIdx, cd.elemIdx,
            runs[cd.runIdx].count, runs[cd.runIdx].nonEmpty));

        for (long off : kBases) {
            uintptr_t b = (uintptr_t)((long long)cd.at + off);
            if (!Readable(b, kContSize)) {
                AppendReport(rep, Fmt("      base %+5ld  unreadable\r\n", off));
                continue;
            }
            unsigned char c[kContSize];
            if (!SafeRead(b, c, kContSize)) continue;
            int width = 0, height = 0, version = 0;
            unsigned int cls = 0, ssg = 0;
            memcpy(&width, c + kContWidth, 4);
            memcpy(&height, c + kContHeight, 4);
            memcpy(&version, c + kContVersion, 4);
            memcpy(&cls, c + kContClass, 4);
            memcpy(&ssg, c + kContStackGroup, 4);
            std::string nm;
            bool nameOk = ContainerName(b, &nm);
            AppendReport(rep, Fmt(
                "      base %+5ld  w=%-6d h=%-6d ver=%-8d cls=%-6u ssg=%-6u cool=%u "
                "name=%s\r\n",
                off, width, height, version, cls, ssg, c[kContIsCool],
                nameOk ? (nm.empty() ? "(empty)" : nm.c_str()) : "(not a string)"));
        }

        // And the raw neighbourhood, which is what actually settles it.
        AppendReport(rep, "      raw qwords around the pointer:\r\n");
        for (long off = -0x40; off <= 0x40; off += 8) {
            uintptr_t a = (uintptr_t)((long long)cd.at + off);
            if (!Readable(a, 8)) continue;
            uintptr_t v = 0;
            if (!SafeRead(a, &v, 8)) continue;
            AppendReport(rep, Fmt("        %+4ld  %016llX  %s%s\r\n", off,
                                  (unsigned long long)v, Annotate(v).c_str(),
                                  off == 0 ? "   <== the pointer" : ""));
        }
        AppendReport(rep, "\r\n");
    }

    std::string verdict = good ? "INVENTORY FOUND"
                        : runs.empty() ? "NO INVENTORY FOUND"
                        : "RUNS FOUND, CONTAINER LAYOUT UNCONFIRMED";
    AppendReport(rep, Fmt(
        "VERDICT: %s\r\n"
        "----------------------------------------------------------------\r\n"
        "  %llu element runs, %llu pointers to them, %d containers validated,\r\n"
        "  %llu candidates examined against the container layout\r\n"
        "  probe took %llu ms\r\n",
        verdict.c_str(), (unsigned long long)runs.size(),
        (unsigned long long)handles.size(), good, (unsigned long long)cands.size(),
        (unsigned long long)(GetTickCount64() - t0)));

    WriteReportFile(outPath, rep);
    logger::Pushf(good ? Level::Info : Level::Warn, "instprobe",
                  "verdict %s: %llu element runs, %d containers validated, %.1f MB scanned "
                  "in %llu ms | detail in %s",
                  verdict.c_str(), (unsigned long long)runs.size(), good,
                  (scanned + scannedB) / 1048576.0,
                  (unsigned long long)(GetTickCount64() - t0), ToUtf8(outPath).c_str());

    g_regions.clear();
    g_regions.shrink_to_fit();
    g_exclude.clear();
}


// ---------------------------------------------------------------------------
// STRING HUNT -- does the running game hold item ids as text at all?
//
// Two probes have now failed to find an inventory, and the reason is the same
// each time: both assumed cGcInventoryElement.Id is a 16-byte ASCII item id in
// live memory, because that is what the *serialisation* schema says. Across 7.3 GB
// the only 16-byte upper-case ids found were mission identifiers -- WORMHUNT,
// EGG_PODS, POLO_PROGRESS. Not one substance or product.
//
// The likely explanation is that the class table describes the SAVE format, and
// the runtime interns item ids as hashes instead of strings. That would be an
// entirely ordinary engine choice, and it would mean no amount of adjusting
// thresholds on the old signature can ever work.
//
// This settles it, and carries its own control. The product tables in .rdata must
// contain these ids as literal text -- the game has to read them from the MBINs at
// some point -- so a hit in the image proves the search works. A hit in the image
// and none on the heap is then a real answer rather than a broken search:
//
//   image + heap  -> ids are text at runtime; dump around a heap hit to find the
//                    real live layout, which is not the serialised one
//   image only    -> ids are hashed at runtime. Stop scanning for leaves, find the
//                    root object and chase pointers instead
//   neither       -> the search itself is broken, and nothing else here means
//                    anything
constexpr int kMaxHuntNames = 128;
// Six was too few. Run 1 found six heap hits for every id and all six came from
// one place -- two from the probe's own stack copy of the names file, four from a
// single 0x10-stride lookup table -- so the quota was exhausted before anything
// interesting could be reached. The cap exists to bound the report, not the search.
constexpr int kMaxHuntHitsPerName = 32;

// The patterns live here, in one static block, for one reason: a hunt that stores
// the strings it is looking for in freshly allocated memory will find them. Run 1
// did exactly that -- two of the six RED2 hits were the probe's own 8 KB file
// buffer, with "# Real internal ids, cross-checked against" legible beside them.
// One static arena is one range to exclude.
char g_huntArena[kMaxHuntNames * 32];
size_t g_huntArenaUsed = 0;

// One pass over memory testing every pattern at each position, instead of one pass
// per pattern. The first version took nineteen minutes on eighteen names because it
// re-walked all 8 GB for each one; cost scaled with the number of names, which is
// exactly backwards for a list meant to grow.
//
// Nearly every position is rejected by a single table lookup: only bytes that begin
// some pattern are worth a memcmp, and in a binary that is a small fraction.
struct HuntSet {
    const char* pat[kMaxHuntNames];
    int         len[kMaxHuntNames];
    int         count;
    // For each possible first byte, which patterns start with it.
    unsigned char byFirst[256][16];
    unsigned char nByFirst[256];
};

void BuildHuntSet(HuntSet* hs, const std::vector<std::string>& names) {
    memset(hs->nByFirst, 0, sizeof(hs->nByFirst));
    hs->count = 0;
    g_huntArenaUsed = 0;
    for (auto& n : names) {
        if (n.empty() || hs->count >= kMaxHuntNames) continue;
        if (n.size() + 1 > sizeof(g_huntArena) - g_huntArenaUsed) continue;
        unsigned char c0 = (unsigned char)n[0];
        if (hs->nByFirst[c0] >= 16) continue;      // 16 patterns per first byte is plenty
        // Copy into the arena rather than pointing at the caller's std::string, whose
        // buffer is heap memory this very scan will walk over.
        char* slot = g_huntArena + g_huntArenaUsed;
        memcpy(slot, n.c_str(), n.size() + 1);
        g_huntArenaUsed += n.size() + 1;
        hs->pat[hs->count] = slot;
        hs->len[hs->count] = (int)n.size();
        hs->byFirst[c0][hs->nByFirst[c0]++] = (unsigned char)hs->count;
        ++hs->count;
    }
}

// POD-only and SEH-guarded, like every other scan here.
int ScanForPatterns(const HuntSet* hs, uintptr_t base, size_t size,
                    uintptr_t* hitAt, int* hitPat, int outMax,
                    int* faulted, size_t* scanned) {
    int n = 0;
    *faulted = 0;
    *scanned = 0;
    if (outMax <= 0 || size < 8) return 0;
    const unsigned char* p = (const unsigned char*)base;
    const unsigned char* end = (const unsigned char*)(base + size) - 32;
    __try {
        for (; p < end; ++p) {
            unsigned char c0 = *p;
            int cnt = hs->nByFirst[c0];
            if (!cnt) continue;
            for (int k = 0; k < cnt; ++k) {
                int pi = hs->byFirst[c0][k];
                int len = hs->len[pi];
                if (memcmp(p, hs->pat[pi], (size_t)len) != 0) continue;
                if (p[len] != '\0') continue;                  // whole token only
                unsigned char b = (p > (const unsigned char*)base) ? p[-1] : 0;
                if (b && ((b >= 'A' && b <= 'Z') || (b >= 'a' && b <= 'z') ||
                          (b >= '0' && b <= '9') || b == '_' || b == '^')) continue;
                hitAt[n] = (uintptr_t)p;
                hitPat[n] = pi;
                ++n;
                break;
            }
            if (n >= outMax) break;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        *faulted = 1;
    }
    *scanned = (size_t)((const unsigned char*)p - (const unsigned char*)base);
    return n;
}

// How far apart are the id strings around this hit?
//
// This is the distinction run 1 could not draw. Every one of its heap hits was an
// id 0x10 bytes from the next id and nothing else between them -- a plain lookup
// table of every substance name in the game, which exists whether or not the
// player owns any of them. A live inventory slot is 0x30 apart with numbers in
// between. Both are "an id as text on the heap", and only the stride separates
// them, so a report that prints hex without the stride leaves the actual question
// open. Returns the smallest stride that has an id neighbour, or 0 for none.
// An id at exactly this address, tolerating the '^' that marks an installed
// technology. ReadIdentifierRaw rejects '^' -- correctly, for its own callers --
// and without this the stride of an element array whose neighbour happens to be
// a technology came back as a multiple of the true one.
bool IdentifierAt(uintptr_t a) {
    char buf[24];
    if (!Readable(a, 18)) return false;
    // It must START here, not merely continue here. Without this the stride test
    // matches the TAIL of a neighbouring id: the control put ^LAUNCHFUEL 0x30
    // before FERRITE_DUST and 0x28 before it landed on "UEL", so a 0x30 element
    // array was measured as 0x28. Every stride in a real save would have been
    // just as arbitrary. Same whole-token rule the pattern scanner uses.
    if (a > 0x10000ull) {
        unsigned char before = 0;
        if (SafeRead(a - 1, &before, 1) && before &&
            ((before >= 'A' && before <= 'Z') || (before >= 'a' && before <= 'z') ||
             (before >= '0' && before <= '9') || before == '_' || before == '^'))
            return false;
    }
    if (ReadIdentifierRaw(a, buf, 17) >= 2) return true;
    unsigned char c = 0;
    if (!SafeRead(a, &c, 1) || c != '^') return false;
    return ReadIdentifierRaw(a + 1, buf, 17) >= 2;
}

int NeighbourStride(uintptr_t at) {
    static const int kTry[] = { 0x10, 0x18, 0x20, 0x28, 0x30, 0x38,
                                0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70 };
    for (int i = 0; i < (int)(sizeof(kTry) / sizeof(kTry[0])); ++i) {
        int s = kTry[i];
        if (IdentifierAt(at + (uintptr_t)s)) return s;
        if (at > (uintptr_t)s && IdentifierAt(at - (uintptr_t)s)) return s;
    }
    return 0;
}

uintptr_t g_huntFileLo = 0, g_huntFileHi = 0, g_huntTextLo = 0, g_huntTextHi = 0;

std::vector<std::string> ReadHuntList() {
    std::vector<std::string> out;
    std::wstring path = g_outDir + L"probe_strings.txt";
    HANDLE h = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h != INVALID_HANDLE_VALUE) {
        // Both buffers are static and their ranges are excluded below. A local
        // `char buf[8192]` here is what produced run 1's phantom RED2 hits: the
        // ids went on the probe thread's own stack and the scan found them there.
        static std::string text;
        static char buf[8192];
        text.clear();
        DWORD got = 0;
        while (ReadFile(h, buf, sizeof(buf), &got, nullptr) && got) text.append(buf, got);
        CloseHandle(h);
        g_huntFileLo = (uintptr_t)buf;
        g_huntFileHi = (uintptr_t)buf + sizeof(buf);
        g_huntTextLo = (uintptr_t)text.data();
        g_huntTextHi = (uintptr_t)text.data() + text.capacity() + 1;
        size_t i = 0;
        while (i < text.size() && (int)out.size() < kMaxHuntNames) {
            size_t e = text.find_first_of("\r\n", i);
            if (e == std::string::npos) e = text.size();
            std::string line = text.substr(i, e - i);
            i = e + 1;
            size_t a = line.find_first_not_of(" \t");
            if (a == std::string::npos) continue;
            size_t b = line.find_last_not_of(" \t");
            line = line.substr(a, b - a + 1);
            if (!line.empty() && line[0] != '#') out.push_back(line);
        }
    }
    if (out.empty()) {
        // Ordinary early-game substances and technologies. A wrong guess costs a
        // "not found" line, so the list is broad rather than careful.
        static const char* kDefaults[] = {
            "CARBON", "OXYGEN", "SODIUM", "FERRITE_DUST", "PURE_FERRITE",
            "MAGNETISED_FERRITE", "TRITIUM", "DI_HYDROGEN", "CHROMATIC_METAL",
            "COPPER", "GOLD", "SILVER", "PLATINUM", "PUGNEUM", "SALT",
            "LAUNCHFUEL", "^LAUNCHFUEL", "^CARBON", "^OXYGEN", "^TRITIUM",
            "LAUNCHSUB1", "SHIPJUMP1", "HYPERDRIVE", "PROTECT", "HEALTH",
        };
        for (const char* d : kDefaults) out.push_back(d);
    }
    return out;
}

void RunStringHunt(int runNo) {
    ULONGLONG t0 = GetTickCount64();
    std::string rep;
    std::wstring outPath = g_outDir + L"stringhunt_" + std::to_wstring(runNo) + L".txt";

    ReadImageInfo();
    BuildRegionMap();
    ULONG_PTR stackLo = 0, stackHi = 0;
    GetCurrentThreadStackLimits(&stackLo, &stackHi);

    std::vector<std::string> names = ReadHuntList();
    AppendReport(rep, Fmt(
        "NMS string hunt -- run %d, hook " NMSLOG_HOOK_VERSION "\r\n"
        "================================================================\r\n"
        "Are item ids held as text in the running game? The product tables in\r\n"
        ".rdata must contain them, so an image hit proves the search works. An\r\n"
        "image hit with nothing on the heap means the runtime interns them as\r\n"
        "something other than text, and scanning for leaf objects cannot work.\r\n\r\n"
        "build:   NMS.exe base 0x%llX  timestamp 0x%08lX\r\n"
        "hunting: %llu strings\r\n\r\n",
        runNo, (unsigned long long)g_img.base, g_img.timeStamp,
        (unsigned long long)names.size()));

    struct HuntHit { uintptr_t at; int nameIdx; bool image; int stride; };
    std::vector<HuntHit> hits;
    size_t scanned = 0, sinceYield = 0;
    size_t budget = (size_t)g_config.instProbeBudgetMB << 20;

    HuntSet hs;
    BuildHuntSet(&hs, names);

    // Everything holding a copy of what we are hunting for. Run 1 skipped this step
    // entirely -- RunStringHunt never touched g_exclude -- and duly reported the
    // probe's own file buffer as a heap hit, twice.
    g_exclude.clear();
    Exclude((uintptr_t)g_huntArena, (uintptr_t)g_huntArena + sizeof(g_huntArena));
    Exclude(g_huntFileLo, g_huntFileHi);
    Exclude(g_huntTextLo, g_huntTextHi);
    Exclude((uintptr_t)stackLo, (uintptr_t)stackHi);
    Exclude((uintptr_t)names.data(),
            (uintptr_t)names.data() + names.size() * sizeof(std::string));
    for (auto& n : names)                      // long names allocate outside the vector
        Exclude((uintptr_t)n.data(), (uintptr_t)n.data() + n.capacity() + 1);

    // 8 GB at 12 MB/s in run 1 is ~80 ns per byte, which no table-lookup loop costs.
    // The suspect is paging: every byte of the game's private memory gets touched,
    // most of it cold. A fault count settles that instead of guessing at the loop.
    PROCESS_MEMORY_COUNTERS pmc0 = {};
    pmc0.cb = sizeof(pmc0);
    GetProcessMemoryInfo(GetCurrentProcess(), &pmc0, sizeof(pmc0));
    ULONGLONG scanMs = 0;
    constexpr int kHitsPerRegion = 4096;
    std::vector<uintptr_t> at(kHitsPerRegion);
    std::vector<int> pat(kHitsPerRegion);

    for (int phase = 0; phase < 2; ++phase) {          // 0 = image (the control), 1 = heap
        for (size_t ri = 0; ri < g_regions.size(); ++ri) {
            const Region& r = g_regions[ri];
            bool isImage = r.type == MEM_IMAGE;
            if (phase == 0 && !isImage) continue;
            if (phase == 1) {
                if (r.type != MEM_PRIVATE) continue;
                if ((uintptr_t)stackLo >= r.base && (uintptr_t)stackLo < r.base + r.size) continue;
                if (scanned >= budget) break;
            }
            int faulted = 0;
            size_t did = 0;
            ULONGLONG ts = GetTickCount64();
            int n = ScanForPatterns(&hs, r.base, r.size, at.data(), pat.data(),
                                    kHitsPerRegion, &faulted, &did);
            scanMs += GetTickCount64() - ts;
            for (int k = 0; k < n; ++k) {
                if (Excluded(at[k])) continue;          // our own copy of the pattern
                int already = 0;
                for (auto& h : hits)
                    if (h.nameIdx == pat[k] && h.image == isImage) ++already;
                if (already < kMaxHuntHitsPerName)
                    hits.push_back(HuntHit{at[k], pat[k], isImage, 0});
            }
            scanned += did;
            sinceYield += did;
            if (sinceYield >= kThrottleEvery) { sinceYield = 0; Sleep(1); }
        }
    }

    int inImage = 0, inHeap = 0, namesInImage = 0, namesInHeap = 0;
    for (size_t ni = 0; ni < names.size(); ++ni) {
        int im = 0, hp = 0;
        for (auto& h : hits) {
            if (h.nameIdx != (int)ni) continue;
            if (h.image) ++im; else ++hp;
        }
        inImage += im;
        inHeap += hp;
        if (im) ++namesInImage;
        if (hp) ++namesInHeap;
        AppendReport(rep, Fmt("  %-22s image:%-3d heap:%d\r\n", names[ni].c_str(), im, hp));
    }

    AppendReport(rep, Fmt(
        "\r\n  %d of %llu found in the image, %d of %llu on the heap "
        "(%d and %d hits)\r\n  scanned %.1f MB in one pass over all patterns\r\n\r\n",
        namesInImage, (unsigned long long)names.size(), namesInHeap,
        (unsigned long long)names.size(), inImage, inHeap, scanned / 1048576.0));

    PROCESS_MEMORY_COUNTERS pmc1 = {};
    pmc1.cb = sizeof(pmc1);
    GetProcessMemoryInfo(GetCurrentProcess(), &pmc1, sizeof(pmc1));
    AppendReport(rep, Fmt(
        "  cost: %llu ms scanning, %lu page faults during the run\r\n"
        "        (%.1f MB/s -- if the faults are in the millions the loop is not the\r\n"
        "         bottleneck and touching every byte of 8 GB simply costs this much)\r\n\r\n",
        (unsigned long long)scanMs,
        (unsigned long)(pmc1.PageFaultCount - pmc0.PageFaultCount),
        scanMs ? (scanned / 1048576.0) / (scanMs / 1000.0) : 0.0));

    // What each heap hit SITS IN, before any hex. Run 1 printed six dumps and no
    // table, so the fact that every hit came from one lookup table had to be
    // reconstructed by hand from the bytes. The stride says it in a word.
    int nPooled = 0, nElem = 0, nLoose = 0;
    for (auto& h : hits) {
        if (h.image) continue;
        h.stride = NeighbourStride(h.at);
        if (h.stride == 0x10) ++nPooled;
        else if (h.stride == (int)kElemStride) ++nElem;
        else ++nLoose;
    }
    AppendReport(rep, Fmt("  %d heap hits: %d in 0x10 id pools, %d at the 0x%X element "
                          "stride, %d elsewhere\r\n",
                          nPooled + nElem + nLoose, nPooled, nElem,
                          (unsigned)kElemStride, nLoose));
    for (auto& h : hits) {
        if (h.image) continue;
        const char* what = h.stride == 0x10   ? "id pool -- a table of every id, not an inventory"
                         : h.stride == 0      ? "no id neighbour at any stride"
                         : h.stride == (int)kElemStride ? "*** element stride ***" : "other";
        AppendReport(rep, Fmt("    0x%012llX  %-20s stride 0x%02X  %s\r\n",
                              (unsigned long long)h.at, names[h.nameIdx].c_str(),
                              (unsigned)h.stride, what));
    }
    AppendReport(rep, "\r\n");

    // Dump the informative ones first. A seventh dump of the id pool teaches nothing;
    // one hit at an unexplained stride is the whole lead.
    int shown = 0;
    for (int pass = 0; pass < 3 && shown < 10; ++pass) {
    for (auto& h : hits) {
        if (h.image || shown >= 10) continue;
        bool isElem = h.stride == (int)kElemStride;
        bool isPool = h.stride == 0x10;
        if (pass == 0 && !isElem) continue;
        if (pass == 1 && (isElem || isPool)) continue;
        if (pass == 2 && !isPool) continue;
        ++shown;
        AppendReport(rep, Fmt("  --- heap hit: \"%s\" at 0x%llX  (stride 0x%02X)\r\n",
                              names[h.nameIdx].c_str(), (unsigned long long)h.at,
                              (unsigned)h.stride));
        for (long off = -0x30; off <= 0x40; off += 8) {
            uintptr_t a = (uintptr_t)((long long)h.at + off);
            if (!Readable(a, 8)) continue;
            uintptr_t v = 0;
            if (!SafeRead(a, &v, 8)) continue;
            unsigned int lo = (unsigned)(v & 0xFFFFFFFF), hi = (unsigned)(v >> 32);
            AppendReport(rep, Fmt("      %+4ld  %016llX  u32 %-11u %-11u %s%s\r\n",
                                  off, (unsigned long long)v, lo, hi,
                                  Annotate(v).c_str(), off == 0 ? "  <== the string" : ""));
        }
        AppendReport(rep, "\r\n");
    }
    }

    const char* verdict = (!namesInImage && !namesInHeap) ? "SEARCH FOUND NOTHING AT ALL"
                        : !namesInHeap                     ? "IDS ARE NOT TEXT AT RUNTIME"
                        : nElem                            ? "ELEMENT-STRIDE HITS FOUND"
                        : "IDS ARE TEXT, BUT ONLY IN LOOKUP TABLES";
    const char* meaning =
        (!namesInImage && !namesInHeap)
            ? "  Not even the image matched, so the search is broken or the id list is\r\n"
              "  wrong. Nothing else in this report means anything until that is fixed.\r\n"
        : (nElem
            ? "  At least one id sits at the serialised element stride, so a live slot\r\n"
              "  array is in reach. The starred dumps above are the candidates.\r\n"
          : namesInHeap
            ? "  Ids are text on the heap, but every hit is 0x10 from the next id with\r\n"
              "  nothing in between: a lookup table of every id in the game, which is\r\n"
              "  present whether or not the player owns any of them. That answers the\r\n"
              "  interning question and NOT the inventory one. Widen the id list, or\r\n"
              "  stop scanning leaves and chase pointers from cGcPlayerStateData.\r\n"
            : "  The image has them and the heap does not: the runtime interns item ids\r\n"
              "  as something other than text, so the class table describes the save\r\n"
              "  format rather than the live object. Scanning for leaf objects cannot\r\n"
              "  work. Find a root object and chase pointers, or hook a function.\r\n");

    AppendReport(rep, Fmt(
        "VERDICT: %s\r\n"
        "----------------------------------------------------------------\r\n%s"
        "\r\n  hunt took %llu ms\r\n",
        verdict, meaning, (unsigned long long)(GetTickCount64() - t0)));

    WriteReportFile(outPath, rep);
    logger::Pushf(Level::Info, "stringhunt",
                  "verdict %s: %d/%llu ids in the image, %d/%llu on the heap, %.1f MB "
                  "scanned in %llu ms | detail in %s",
                  verdict, namesInImage, (unsigned long long)names.size(), namesInHeap,
                  (unsigned long long)names.size(), scanned / 1048576.0,
                  (unsigned long long)(GetTickCount64() - t0), ToUtf8(outPath).c_str());

    g_regions.clear();
    g_regions.shrink_to_fit();
}

// One "probe run" is whichever probes are switched on, in the order that makes
// sense: the metadata scan describes shapes, the instance scan uses them.
void RunAll(int runNo) {
    if (g_config.metaProbe) RunProbe(runNo);
    if (g_config.instProbe) RunInstanceProbe(runNo);
    if (g_config.stringHunt) RunStringHunt(runNo);
}

// Set from the file hooks the instant a save finishes. If cGcPlayerStateData is
// the save document then this is the only moment it is fully materialised, so a
// probe armed here sees what a mid-game probe cannot.
volatile LONG g_saveArmed = 0;

DWORD WINAPI ProbeThread(LPVOID) {
    // Both halves matter: our own file hooks must not observe the probe's I/O,
    // and the vectored handler in crash.cpp must not log the faults the probe
    // provokes on purpose. t_inHook is what both of those check.
    t_inHook = true;

    int runNo = 0;
    std::wstring sentinel = g_outDir + L"probe.now";
    int delay = g_config.metaProbeDelaySeconds;

    // Wait before the first run: the metadata cannot be in memory before the game
    // has loaded far enough to need it.
    for (int waited = 0; delay > 0 && waited < delay * 4; ++waited) {
        if (GetFileAttributesW(sentinel.c_str()) != INVALID_FILE_ATTRIBUTES) break;
        Sleep(250);
    }
    if (delay > 0) RunAll(++runNo);

    // Then on demand, so one session can be probed again after a save is loaded --
    // which is when the inventory containers actually exist.
    for (;;) {
        if (GetFileAttributesW(sentinel.c_str()) != INVALID_FILE_ATTRIBUTES) {
            DeleteFileW(sentinel.c_str());
            RunAll(++runNo);
        } else if (InterlockedExchange(&g_saveArmed, 0)) {
            logger::Push(Level::Info, "metaprobe",
                         "a save was just written -- probing now, which is when the save "
                         "document is materialised");
            RunAll(++runNo);
        }
        Sleep(1000);
    }
}

} // namespace

namespace metaprobe {

void OnSaveWritten() {
    if (g_config.instProbeOnSave) InterlockedExchange(&g_saveArmed, 1);
}

void Start() {
    if (!g_config.metaProbe && !g_config.instProbe && !g_config.stringHunt) return;
    logger::Pushf(Level::Info, "metaprobe",
                  "probe armed (metadata=%d instances=%d): first run in %ds, then on "
                  "demand (drop a file named probe.now in %s to re-run)",
                  g_config.metaProbe, g_config.instProbe,
                  g_config.metaProbeDelaySeconds, ToUtf8(g_outDir).c_str());
    HANDLE t = CreateThread(nullptr, 0, ProbeThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
}

} // namespace metaprobe

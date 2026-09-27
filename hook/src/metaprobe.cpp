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

    int direct = FindContainers(rep);
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

    for (size_t i = 0; i < runs.size() && i < 40; ++i)
        AppendReport(rep, Fmt("  run %2llu: 0x%llX  %d slots, %d occupied\r\n",
                              (unsigned long long)i, (unsigned long long)runs[i].base,
                              runs[i].count, runs[i].nonEmpty));
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

// One "probe run" is whichever probes are switched on, in the order that makes
// sense: the metadata scan describes shapes, the instance scan uses them.
void RunAll(int runNo) {
    if (g_config.metaProbe) RunProbe(runNo);
    if (g_config.instProbe) RunInstanceProbe(runNo);
}

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
        }
        Sleep(1000);
    }
}

} // namespace

namespace metaprobe {

void Start() {
    if (!g_config.metaProbe && !g_config.instProbe) return;
    logger::Pushf(Level::Info, "metaprobe",
                  "probe armed (metadata=%d instances=%d): first run in %ds, then on "
                  "demand (drop a file named probe.now in %s to re-run)",
                  g_config.metaProbe, g_config.instProbe,
                  g_config.metaProbeDelaySeconds, ToUtf8(g_outDir).c_str());
    HANDLE t = CreateThread(nullptr, 0, ProbeThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
}

} // namespace metaprobe

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
    if (delay > 0) RunProbe(++runNo);

    // Then on demand, so one session can be probed again after a save is loaded --
    // which is when the inventory containers actually exist.
    for (;;) {
        if (GetFileAttributesW(sentinel.c_str()) != INVALID_FILE_ATTRIBUTES) {
            DeleteFileW(sentinel.c_str());
            RunProbe(++runNo);
        }
        Sleep(1000);
    }
}

} // namespace

namespace metaprobe {

void Start() {
    if (!g_config.metaProbe) return;
    logger::Pushf(Level::Info, "metaprobe",
                  "metadata probe armed: first run in %ds, then on demand "
                  "(drop a file named probe.now in %s to re-run)",
                  g_config.metaProbeDelaySeconds, ToUtf8(g_outDir).c_str());
    HANDLE t = CreateThread(nullptr, 0, ProbeThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
}

} // namespace metaprobe

#include "common.h"
#include <cwctype>
#include <cctype>
#include <cstdio>

std::string ToUtf8(const wchar_t* s, int len) {
    if (!s) return {};
    if (len < 0) len = (int)wcslen(s);
    if (len == 0) return {};
    int n = WideCharToMultiByte(CP_UTF8, 0, s, len, nullptr, 0, nullptr, nullptr);
    std::string out((size_t)n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, s, len, out.data(), n, nullptr, nullptr);
    return out;
}

std::string ToUtf8(const std::wstring& s) { return ToUtf8(s.c_str(), (int)s.size()); }

std::string DescribeAddress(const void* addr) {
    char buf[MAX_PATH + 64];
    HMODULE mod = nullptr;
    if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCWSTR)addr, &mod) && mod) {
        wchar_t path[MAX_PATH];
        DWORD n = GetModuleFileNameW(mod, path, MAX_PATH);
        const wchar_t* name = path;
        for (DWORD i = 0; i < n; ++i)
            if (path[i] == L'\\' || path[i] == L'/') name = path + i + 1;
        snprintf(buf, sizeof(buf), "%s+0x%llX", ToUtf8(name).c_str(),
                 (unsigned long long)((const char*)addr - (const char*)mod));
    } else {
        snprintf(buf, sizeof(buf), "0x%p (no module)", addr);
    }
    return buf;
}

bool ContainsNoCase(const wchar_t* hay, const wchar_t* needle) {
    if (!hay || !needle) return false;
    size_t nl = wcslen(needle);
    for (const wchar_t* p = hay; *p; ++p) {
        size_t i = 0;
        while (i < nl && p[i] && towupper(p[i]) == towupper(needle[i])) ++i;
        if (i == nl) return true;
    }
    return false;
}

bool EndsWithNoCase(const wchar_t* hay, const wchar_t* needle) {
    if (!hay || !needle) return false;
    size_t hl = wcslen(hay), nl = wcslen(needle);
    return hl >= nl && _wcsicmp(hay + hl - nl, needle) == 0;
}

static bool HasWord(const std::string& lower, const char* w) { return lower.find(w) != std::string::npos; }

Level ClassifyText(const char* text, size_t len, Level fallback) {
    std::string lower(text, len);
    for (auto& c : lower) c = (char)tolower((unsigned char)c);
    if (HasWord(lower, "fatal") || HasWord(lower, "crash") || HasWord(lower, "exception"))
        return Level::Error;
    if (HasWord(lower, "error") || HasWord(lower, "fail") || HasWord(lower, "unable") ||
        HasWord(lower, "assert") || HasWord(lower, "invalid") || HasWord(lower, "missing") ||
        HasWord(lower, "corrupt") || HasWord(lower, "warning") || HasWord(lower, "cannot"))
        return Level::Warn;
    return fallback;
}

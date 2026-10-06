// Portable platform substitutes. Namespace, node, sector, callback, and comparison
// algorithms are extracted from the current production sources by run.py.
#include <algorithm>
#include <atomic>
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <cwctype>
#include <iostream>
#include <iterator>
#include <map>
#include <memory>
#include <mutex>
#include <new>
#include <optional>
#include <shared_mutex>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>
#include <unordered_set>
#define __FUNCTION__ "portable"
using std::min;
using UINT64 = uint64_t;
using UINT32 = uint32_t;
using UINT16 = uint16_t;
using INT32 = int32_t;
using ULONG = uint32_t;
using SIZE_T = size_t;
using ULONG_PTR = uintptr_t;
using NTSTATUS = int;
using BOOLEAN = bool;
using byte = unsigned char;
using WCHAR = wchar_t;
using PWSTR = wchar_t*;
using PCWSTR = const wchar_t*;
using PCSTR = const char*;
using PVOID = void*;
using PULONG = ULONG*;
using PSIZE_T = SIZE_T*;
constexpr bool FALSE = false;
constexpr int STATUS_SUCCESS = 0, STATUS_OBJECT_NAME_COLLISION = -1,
    STATUS_ACCESS_DENIED = -2, STATUS_OBJECT_NAME_INVALID = -3,
    STATUS_INSUFFICIENT_RESOURCES = -4, STATUS_OBJECT_NAME_NOT_FOUND = -5,
    STATUS_OBJECT_PATH_NOT_FOUND = -6, STATUS_NOT_A_DIRECTORY = -7,
    STATUS_DISK_FULL = -8, STATUS_END_OF_FILE = -9, STATUS_UNSUCCESSFUL = -10;
constexpr unsigned FILE_ATTRIBUTE_DIRECTORY = 16, FILE_ATTRIBUTE_ARCHIVE = 32,
    FILE_ATTRIBUTE_REPARSE_POINT = 1024, FILE_DIRECTORY_FILE = 1, FILE_NEED_EA = 128;
constexpr int MEM_COMMIT = 1, MEM_RESERVE = 2, PAGE_READWRITE = 4, MEM_RELEASE = 8,
    EVENTLOG_ERROR_TYPE = 1, EVENTLOG_INFORMATION_TYPE = 2, EVENTLOG_WARNING_TYPE = 3,
    LOCALE_INVARIANT = 0, NORM_IGNORECASE = 1;
#define NT_SUCCESS(x) ((x) >= 0)
#define FIELD_OFFSET(T, field) offsetof(T, field)
struct SECURITY_DESCRIPTOR { uint32_t value; };
using PSECURITY_DESCRIPTOR = SECURITY_DESCRIPTOR*;
struct FILE_FULL_EA_INFORMATION {
    ULONG NextEntryOffset; byte Flags, EaNameLength; UINT16 EaValueLength; char EaName[1];
};
using PFILE_FULL_EA_INFORMATION = FILE_FULL_EA_INFORMATION*;
struct FSP_FSCTL_FILE_INFO {
    UINT32 FileAttributes = 0, ReparseTag = 0, EaSize = 0;
    UINT64 FileSize = 0, AllocationSize = 0, CreationTime = 0, LastAccessTime = 0,
        LastWriteTime = 0, ChangeTime = 0, IndexNumber = 0;
};
struct FSP_FSCTL_OPEN_FILE_INFO { WCHAR NormalizedName[32766]; UINT16 NormalizedNameSize = 65528; };
struct FSP_FILE_SYSTEM { PVOID UserContext; };
static FSP_FSCTL_OPEN_FILE_INFO normalizedInfo;
inline auto FspFileSystemGetOpenFileInfo(FSP_FSCTL_FILE_INFO*) { return &normalizedInfo; }
inline size_t GetSecurityDescriptorLength(PSECURITY_DESCRIPTOR) { return sizeof(SECURITY_DESCRIPTOR); }
inline int memcpy_s(void* out, size_t capacity, const void* in, size_t count) {
    assert(count <= capacity); std::memcpy(out, in, count); return 0;
}
inline int wcscpy_s(wchar_t* out, size_t capacity, const wchar_t* in) {
    assert(std::wcslen(in) < capacity); std::wcscpy(out, in); return 0;
}
template <typename... T> void FspDebugLog(T...) {}
template <typename... T> void FspServiceLog(T...) {}
inline long InterlockedIncrement(volatile long* value) { long result = *value + 1; *value = result; return result; }
inline long InterlockedDecrement(volatile long* value) { long result = *value - 1; *value = result; return result; }
inline INT32 FspInterlockedLoad32(volatile INT32* value) { return *value; }
inline UINT64 InterlockedExchangeAdd(volatile UINT64* value, UINT64 delta) {
    UINT64 old = *value; *value = old + delta; return old;
}
inline void InterlockedExchangeSubtract(volatile UINT64* value, UINT64 delta) { *value = *value - delta; }
inline ULONG FspFileSystemGetEaPackedSize(PFILE_FULL_EA_INFORMATION ea) {
    return offsetof(FILE_FULL_EA_INFORMATION, EaName) + ea->EaNameLength + 1 + ea->EaValueLength;
}
inline NTSTATUS FspFileSystemEnumerateEa(FSP_FILE_SYSTEM* fs,
    NTSTATUS (*callback)(FSP_FILE_SYSTEM*, PVOID, PFILE_FULL_EA_INFORMATION),
    PVOID node, PFILE_FULL_EA_INFORMATION ea, ULONG) { return callback(fs, node, ea); }
inline int lstrlenW(PCWSTR name) { return std::wcslen(name); }
inline int _wcsnicmp(PCWSTR a, PCWSTR b, int length) {
    for (int i = 0; i < length; ++i) {
        int diff = std::towlower(a[i]) - std::towlower(b[i]);
        if (diff || !a[i]) return diff;
    }

    return 0;
}
inline int CompareStringW(int, int, PCWSTR a, int alen, PCWSTR b, int blen) {
    int diff = _wcsnicmp(a, b, min(alen, blen));
    if (!diff) diff = alen - blen;
    return diff < 0 ? 1 : diff > 0 ? 3 : 2;
}
inline int _stricmp(PCSTR a, PCSTR b) {
    while (*a && std::tolower(*a) == std::tolower(*b)) { ++a; ++b; }
    return std::tolower(*a) - std::tolower(*b);
}
inline int CompareStringA(int, int, PCSTR a, int, PCSTR b, int) {
    int diff = _stricmp(a, b); return diff < 0 ? 1 : diff > 0 ? 3 : 2;
}

// Fail every ordinary C++ allocation in turn. Disable failure after it fires so
// status handling and cleanup can proceed. VirtualAlloc has a separate fault knob.
static long allocationFailure = -1;
static bool allocationFailed = false;
void* operator new(size_t size) {
    if (allocationFailure == 0) {
        allocationFailure = -1; allocationFailed = true; throw std::bad_alloc();
    }

    if (allocationFailure > 0) --allocationFailure;
    if (void* result = std::malloc(size ? size : 1)) return result;
    throw std::bad_alloc();
}
void* operator new[](size_t size) { return ::operator new(size); }
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }
void operator delete(void* value, size_t) noexcept { std::free(value); }
void operator delete[](void* value, size_t) noexcept { std::free(value); }
static int virtualAllocationFailure = -1;
inline void* VirtualAlloc(void*, size_t length, int, int) {
    if (virtualAllocationFailure == 0) { virtualAllocationFailure = -1; return nullptr; }

    if (virtualAllocationFailure > 0) --virtualAllocationFailure;
    return std::calloc(1, length);
}
inline bool VirtualFree(void* value, int, int) { std::free(value); return true; }

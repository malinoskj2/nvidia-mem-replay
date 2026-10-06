// Replay in RAM adapter for WinFsp-MemFs-Extended. GPL-3.0-only.
// Replaces the service CLI with an owned stdin/stdout lifecycle and telemetry.
#include "memfs.h"
#include "exceptions.h"
#include <psapi.h>
#include <cstdio>
#include <cerrno>

static DWORD WINAPI WaitForOwner(void*) {
    char command[512];
    DWORD read;
    const HANDLE input = GetStdHandle(STD_INPUT_HANDLE);

    for (size_t count = 0; count < sizeof(command); ++count) {
        if (!ReadFile(input, &command[count], 1, &read, nullptr) || read == 0 || command[count] == '\n')
            break;
    }

    return 0;
}

static bool Report(Memfs::MemFs& filesystem) {
    MEMORYSTATUSEX memory{};
    memory.dwLength = sizeof(memory);
    if (!GlobalMemoryStatusEx(&memory)) return false;

    PROCESS_MEMORY_COUNTERS counters{};
    char resident[32] = "null";
    if (GetProcessMemoryInfo(GetCurrentProcess(), &counters, sizeof(counters)))
        std::snprintf(resident, sizeof(resident), "%llu", static_cast<unsigned long long>(counters.WorkingSetSize));

    char line[512];
    const int length = std::snprintf(line, sizeof(line),
        "{\"version\":1,\"written_bytes\":%llu,\"buffer_bytes\":%llu,\"resident_bytes\":%s,\"available_bytes\":%llu}\n",
        static_cast<unsigned long long>(filesystem.writtenBytes.load(std::memory_order_relaxed)),
        static_cast<unsigned long long>(filesystem.GetSectorManager().GetAllocatedSectors() * Memfs::FULL_SECTOR_SIZE),
        resident, static_cast<unsigned long long>(memory.ullAvailPhys));

    DWORD written;
    return length > 0 && static_cast<size_t>(length) < sizeof(line) &&
        WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), line, length, &written, nullptr) && written == static_cast<DWORD>(length);
}

int wmain(int argc, wchar_t** argv) {
    // Only the bundled Rust supervisor calls this adapter. Reject ambiguous input.
    if (argc != 5 || wcscmp(argv[1], L"-m") || wcscmp(argv[3], L"-s") ||
        wcslen(argv[2]) != 2 || argv[2][0] < L'D' || argv[2][0] > L'Z' || argv[2][1] != L':' ||
        argv[4][0] < L'0' || argv[4][0] > L'9') return 2;

    wchar_t* end;
    errno = 0;
    const unsigned long long limit = wcstoull(argv[4], &end, 10);
    if (errno || *end || limit < 256000000ULL || limit > 65536000000ULL) return 2;

    // The import library is delay-loaded. Resolve the shared installed WinFsp first.
    if (!NT_SUCCESS(FspLoad(nullptr))) return 3;

    try {
        Memfs::MemFs filesystem(Memfs::MemfsDisk | Memfs::MemfsCaseInsensitive,
            limit, L"NTFS", nullptr, L"NVIDIA Replay", nullptr);
        const auto raw = filesystem.GetRawFileSystem();
        if (!NT_SUCCESS(FspFileSystemSetMountPoint(raw, argv[2]))) return 4;
        if (!NT_SUCCESS(filesystem.Start())) return 5;

        const HANDLE owner = CreateThread(nullptr, 0, WaitForOwner, nullptr, 0, nullptr);
        if (!owner) { filesystem.Stop(); return 6; }

        bool reported;
        do {
            reported = Report(filesystem);
        } while (reported && !filesystem.unexpectedStop.load(std::memory_order_acquire) &&
            WaitForSingleObject(owner, 250) == WAIT_TIMEOUT);

        filesystem.Stop();
        // Dispatch is drained: the final counter includes all completed writes.
        if (reported) Report(filesystem);

        CancelSynchronousIo(owner);
        WaitForSingleObject(owner, 1000);
        CloseHandle(owner);

        if (filesystem.unexpectedStop.load(std::memory_order_acquire)) return 9;
        return reported ? 0 : 7;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "MemFS Extended: %s\n", error.what());
        return 8;
    }
}

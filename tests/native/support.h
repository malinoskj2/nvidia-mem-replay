namespace Memfs {
CreateException::CreateException(NTSTATUS status) : status(status) {}
NTSTATUS CreateException::Which() const { return status; }
const char* CreateException::what() const noexcept { return "create failed"; }
const char* FileNameTooLongException::what() const noexcept { return "name too long"; }
MemFs::MemFs(ULONG flags, UINT64 size, PCWSTR, PCWSTR, PCWSTR, PCWSTR)
    : maxFsSize(size), fileMap(Utils::FileLess{bool(flags & MemfsCaseInsensitive)}) {
    assert(!MEMFS_SINGLETON); MEMFS_SINGLETON = this;
}
MemFs::~MemFs() {
    while (!fileMap.empty()) {
        FileNode* node = fileMap.begin()->second;
        fileMap.erase(fileMap.begin()); node->Dereference();
    }
    assert(sectors.GetAllocatedSectors() == 0); MEMFS_SINGLETON = nullptr;
}
SectorManager& MemFs::GetSectorManager() { return sectors; }
UINT64 MemFs::CalculateMaxTotalSize() { return maxFsSize; }
UINT64 MemFs::CalculateAvailableTotalSize() {
    return maxFsSize - sectors.GetAllocatedSectors() * (sizeof(Sector) + sizeof(Sector*));
}
namespace Utils { uint64_t GetSystemTime() { return 123; } }
namespace Interface {
MemFs* GetMemFs(const FSP_FILE_SYSTEM* fs) { return static_cast<MemFs*>(fs->UserContext); }
FileNode* GetFileNode(PVOID node) { return static_cast<FileNode*>(node); }
}
}

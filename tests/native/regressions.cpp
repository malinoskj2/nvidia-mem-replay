namespace {
constexpr UINT64 TestCapacity = 128 * 1024 * 1024;
constexpr wchar_t SourceName[] = L"\\source-file-with-a-long-name";
constexpr wchar_t TargetName[] = L"\\target-file-with-a-long-name";

struct Fixture {
    MemFs fs;
    FSP_FILE_SYSTEM raw{&fs};
    std::vector<FileNode*> handles;
    explicit Fixture(bool caseInsensitive = false)
        : fs(caseInsensitive ? MemfsCaseInsensitive : 0, TestCapacity, nullptr, nullptr, nullptr, nullptr) {
        add(L"\\", nullptr, FILE_ATTRIBUTE_DIRECTORY);
    }
    ~Fixture() { for (FileNode* node : handles) node->Dereference(); }
    FileNode* add(const std::wstring& name, FileNode* main = nullptr, unsigned attributes = 0) {
        FileNode node(name); node.fileInfo.FileAttributes = attributes;
        node.SetMainNode(main);
        auto [status, stored] = fs.InsertNode(std::move(node));
        assert(status == STATUS_SUCCESS && stored);
        stored->Reference(); handles.push_back(stored); return stored;
    }
    void seed(FileNode* node, byte value) {
        assert(CompatSetFileSizeInternal(&raw, node, FULL_SECTOR_SIZE, false) == STATUS_SUCCESS);
        std::memset(node->GetSectorNode().Sectors[0]->Bytes, value, FULL_SECTOR_SIZE);
    }
    FileNode* find(std::wstring_view name) {
        auto node = fs.FindFile(name); return node ? &node->get() : nullptr;
    }
};
struct Snapshot {
    std::vector<std::wstring> names;
    std::vector<long> refs;
    std::vector<FileNode*> nodes;
    explicit Snapshot(Fixture& fixture) : nodes(fixture.handles) {
        for (FileNode* node : nodes) { names.push_back(node->fileName); refs.push_back(node->GetReferenceCount()); }
    }
    void unchanged(Fixture& fixture) const {
        assert(fixture.fs.EnumerateDescendants(*fixture.handles[0], false).size() == nodes.size());
        for (size_t i = 0; i < nodes.size(); ++i) {
            assert(nodes[i]->fileName == names[i]);
            assert(nodes[i]->GetReferenceCount() == refs[i]);
            assert(fixture.find(names[i]) == nodes[i]);
        }
    }
};
NTSTATUS rename(Fixture& fixture, FileNode* source, const wchar_t* target, bool replace = true) {
    return Interface::Rename(&fixture.raw, source, source->fileName.data(), const_cast<wchar_t*>(target), replace);
}

void replacementAndFailures(bool caseInsensitive) {
    unsigned failures = 0;
    for (int fault = 0; fault < 256; ++fault) {
        Fixture fixture(caseInsensitive);
        FileNode* source = fixture.add(SourceName);
        FileNode* sourceStream = fixture.add(std::wstring(SourceName) + L":meta", source);
        FileNode* sourceOnly = fixture.add(std::wstring(SourceName) + L":source-only", source);
        FileNode* destination = fixture.add(TargetName);
        FileNode* destinationStream = fixture.add(std::wstring(TargetName) + L":meta", destination);
        FileNode* destinationOnly = fixture.add(std::wstring(TargetName) + L":destination-only", destination);
        FileNode* unrelated = fixture.add(L"\\unrelated");
        fixture.seed(sourceStream, 'S'); fixture.seed(destinationStream, 'D');
        Snapshot before(fixture);
        allocationFailed = false; allocationFailure = fault;
        NTSTATUS status = rename(fixture, source, TargetName);
        allocationFailure = -1;
        if (allocationFailed) {
            assert(status == STATUS_INSUFFICIENT_RESOURCES);
            before.unchanged(fixture); ++failures; continue;
        }
        assert(status == STATUS_SUCCESS && failures > 0);
        assert(fixture.find(SourceName) == nullptr);
        assert(fixture.find(TargetName) == source);
        assert(fixture.find(std::wstring(TargetName) + L":meta") == sourceStream);
        assert(fixture.find(std::wstring(TargetName) + L":source-only") == sourceOnly);
        assert(!fixture.find(std::wstring(TargetName) + L":destination-only"));
        assert(fixture.find(L"\\unrelated") == unrelated);
        assert(sourceStream->GetSectorNode().Sectors[0]->Bytes[0] == 'S');
        // Destination main and ADS handles remain usable after their map refs disappear.
        assert(destinationStream->GetMainNode() == destination);
        FSP_FSCTL_FILE_INFO info{}; destinationStream->CopyFileInfo(&info);
        assert(destinationStream->GetSectorNode().Sectors[0]->Bytes[0] == 'D');
        assert(destinationOnly->GetReferenceCount() == 1);
        assert(source->GetReferenceCount() == before.refs[1]);
        assert(destination->GetReferenceCount() == before.refs[4] - 1);
        // Stale cleanup for the replaced handles must not unlink the new owners.
        fixture.fs.RemoveNode(*destination);
        fixture.fs.RemoveNode(*destinationStream);
        assert(fixture.find(TargetName) == source);
        assert(fixture.find(std::wstring(TargetName) + L":meta") == sourceStream);
        std::cout << "rename replacement: " << failures << " allocation failures preserved namespace and refs\n";
        return;
    }
    assert(false && "rename failure sweep never completed");
}

void identityAndDirectories() {
    for (bool caseInsensitive : {false, true}) {
        Fixture fixture(caseInsensitive);
        FileNode* source = fixture.add(SourceName);
        FileNode* stream = fixture.add(std::wstring(SourceName) + L":meta", source);
        Snapshot before(fixture);
        assert(rename(fixture, source, SourceName, false) == STATUS_SUCCESS);
        before.unchanged(fixture);
        assert(rename(fixture, source, L"\\SOURCE-FILE-WITH-A-LONG-NAME", false) == STATUS_SUCCESS);
        assert(fixture.find(L"\\SOURCE-FILE-WITH-A-LONG-NAME") == source);
        assert(stream->fileName == L"\\SOURCE-FILE-WITH-A-LONG-NAME:meta");
    }
    unsigned failures = 0;
    for (int fault = 0; fault < 256; ++fault) {
        Fixture fixture;
        FileNode* directory = fixture.add(L"\\directory", nullptr, FILE_ATTRIBUTE_DIRECTORY);
        FileNode* child = fixture.add(L"\\directory\\child", nullptr, FILE_ATTRIBUTE_DIRECTORY);
        FileNode* file = fixture.add(L"\\directory\\child\\file");
        FileNode* stream = fixture.add(L"\\directory\\child\\file:meta", file);
        fixture.add(L"\\directory-unrelated");
        Snapshot before(fixture);
        allocationFailure = fault; allocationFailed = false;
        NTSTATUS status = rename(fixture, directory, L"\\new-directory", false);
        allocationFailure = -1;
        if (allocationFailed) {
            assert(status == STATUS_INSUFFICIENT_RESOURCES); before.unchanged(fixture); ++failures; continue;
        }
        assert(status == STATUS_SUCCESS && failures > 0);
        assert(fixture.find(L"\\new-directory\\child") == child);
        assert(fixture.find(L"\\new-directory\\child\\file") == file);
        assert(fixture.find(L"\\new-directory\\child\\file:meta") == stream);
        assert(fixture.find(L"\\directory-unrelated")); break;
    }
    Fixture fixture;
    FileNode* source = fixture.add(SourceName);
    fixture.add(std::wstring(SourceName) + L":meta", source);
    fixture.add(TargetName);
    Snapshot before(fixture);
    assert(rename(fixture, source, TargetName, false) == STATUS_OBJECT_NAME_COLLISION);
    before.unchanged(fixture);
    const std::wstring ownStream = std::wstring(SourceName) + L":meta";
    assert(rename(fixture, source, ownStream.c_str()) == STATUS_ACCESS_DENIED);
    before.unchanged(fixture);
    FileNode* ownStreamNode = fixture.find(ownStream);
    assert(rename(fixture, ownStreamNode, SourceName) == STATUS_ACCESS_DENIED);
    before.unchanged(fixture);
    const long mainRefs = source->GetReferenceCount();
    const long streamRefs = ownStreamNode->GetReferenceCount();
    const std::wstring sibling = std::wstring(SourceName) + L":sibling";
    assert(rename(fixture, ownStreamNode, sibling.c_str(), false) == STATUS_SUCCESS);
    assert(ownStreamNode->GetMainNode() == source);
    assert(source->GetReferenceCount() == mainRefs && ownStreamNode->GetReferenceCount() == streamRefs);
    assert(rename(fixture, ownStreamNode, ownStream.c_str(), false) == STATUS_SUCCESS);
    before.unchanged(fixture);
    std::wstring tooLong(MEMFS_MAX_PATH - 3, L'a'); tooLong[0] = L'\\';
    assert(rename(fixture, source, tooLong.c_str()) == STATUS_OBJECT_NAME_INVALID);
    before.unchanged(fixture);
    FileNode* directory = fixture.add(L"\\directory", nullptr, FILE_ATTRIBUTE_DIRECTORY);
    assert(rename(fixture, source, L"\\directory") == STATUS_ACCESS_DENIED);
    assert(rename(fixture, directory, L"\\directory\\child") == STATUS_ACCESS_DENIED);
    // A target stream with no target main still constitutes an unrelated collision.
    fixture.add(L"\\orphan:meta"); Snapshot withOrphan(fixture);
    assert(rename(fixture, source, L"\\orphan") == STATUS_OBJECT_NAME_COLLISION);
    withOrphan.unchanged(fixture);
}

void insertionAndCreateFailures() {
    unsigned insertionFailures = 0;
    for (int fault = 0; fault < 32; ++fault) {
        Fixture fixture;
        FileNode* main = fixture.add(L"\\main");
        const long before = main->GetReferenceCount();
        bool failed = false;
        {
            FileNode stream(L"\\main:stream-with-a-long-name"); stream.SetMainNode(main);
            allocationFailure = fault; allocationFailed = false;
            auto [status, inserted] = fixture.fs.InsertNode(std::move(stream));
            allocationFailure = -1; failed = allocationFailed;
            if (failed) { assert(status == STATUS_INSUFFICIENT_RESOURCES && !inserted); ++insertionFailures; }
            else assert(status == STATUS_SUCCESS && inserted);
        }
        assert(main->GetReferenceCount() == before + (failed ? 0 : 1));
        assert(fixture.find(L"\\main") == main);
        if (!failed) { assert(insertionFailures >= 2); break; }
    }
    for (bool insensitive : {false, true}) {
        unsigned failures = 0;
        for (int fault = 0; fault < 128; ++fault) {
            Fixture fixture(insensitive);
            FileNode* existing = fixture.add(L"\\existing-file");
            wchar_t name[] = L"\\created-file-with-a-long-name";
            SECURITY_DESCRIPTOR security{}; ULONG reparse = 123;
            FSP_FSCTL_FILE_INFO info{};
            PVOID output = existing;
            allocationFailure = fault; allocationFailed = false;
            NTSTATUS status = Interface::Create(&fixture.raw, name, 0, 0, 0, &security,
                FULL_SECTOR_SIZE, &reparse, sizeof(reparse), true, &output, &info);
            allocationFailure = -1;
            if (allocationFailed) {
                assert(status == STATUS_INSUFFICIENT_RESOURCES && output == existing);
                assert(!fixture.find(name)); assert(fixture.find(existing->fileName) == existing);
                assert(fixture.fs.GetSectorManager().GetAllocatedSectors() == 0); ++failures; continue;
            }
            assert(status == STATUS_SUCCESS && output && failures >= 4);
            static_cast<FileNode*>(output)->Dereference();
            std::cout << "create callback: " << failures << " allocation failures returned resources status\n";
            break;
        }
    }
    // New-file EA setup uses the actual node EA implementation, including its map
    // allocations. Every failure must leave all previously published files intact.
    unsigned eaFailures = 0;
    for (int fault = 0; fault < 128; ++fault) {
        Fixture fixture;
        FileNode* existing = fixture.add(L"\\existing-file");
        alignas(FILE_FULL_EA_INFORMATION) byte eaBuffer[64]{};
        auto ea = reinterpret_cast<PFILE_FULL_EA_INFORMATION>(eaBuffer);
        ea->EaNameLength = 4; ea->EaValueLength = 3;
        std::memcpy(ea->EaName, "test\0abc", 8);
        PVOID output = existing; FSP_FSCTL_FILE_INFO info{};
        wchar_t name[] = L"\\created-with-ea-and-a-long-name";
        allocationFailure = fault; allocationFailed = false;
        NTSTATUS status = Interface::Create(&fixture.raw, name, 0, 0, 0, nullptr,
            0, ea, sizeof(eaBuffer), false, &output, &info);
        allocationFailure = -1;
        if (allocationFailed) {
            assert(status == STATUS_INSUFFICIENT_RESOURCES && output == existing);
            assert(!fixture.find(name) && fixture.find(existing->fileName) == existing);
            assert(existing->GetReferenceCount() == 2); ++eaFailures; continue;
        }
        assert(status == STATUS_SUCCESS && eaFailures >= 5);
        auto created = static_cast<FileNode*>(output);
        assert(created->GetEaMap().size() == 1); created->Dereference(); break;
    }
    // Named-stream creation has to release its reference on the existing main
    // node at every failure point, including failure while allocating/mapping it.
    unsigned streamFailures = 0;
    for (int fault = 0; fault < 128; ++fault) {
        Fixture fixture; FileNode* main = fixture.add(L"\\main");
        PVOID output = main; FSP_FSCTL_FILE_INFO info{};
        wchar_t name[] = L"\\main:new-stream-with-a-long-name";
        allocationFailure = fault; allocationFailed = false;
        NTSTATUS status = Interface::Create(&fixture.raw, name, 0, 0, 0, nullptr,
            FULL_SECTOR_SIZE, nullptr, 0, false, &output, &info);
        allocationFailure = -1;
        if (allocationFailed) {
            assert(status == STATUS_INSUFFICIENT_RESOURCES && output == main);
            assert(main->GetReferenceCount() == 2 && !fixture.find(name));
            assert(fixture.fs.GetSectorManager().GetAllocatedSectors() == 0); ++streamFailures; continue;
        }
        assert(status == STATUS_SUCCESS && streamFailures >= 4);
        auto created = static_cast<FileNode*>(output);
        assert(created->GetMainNode() == main); created->Dereference(); break;
    }
    Fixture fixture;
    FileNode* existing = fixture.add(L"\\existing-file");
    const long refs = existing->GetReferenceCount();
    FileNode collision(existing->fileName);
    auto [status, inserted] = fixture.fs.InsertNode(std::move(collision));
    assert(status == STATUS_OBJECT_NAME_COLLISION && !inserted);
    assert(existing->GetReferenceCount() == refs && fixture.find(existing->fileName) == existing);
}

void resizeAndWrite() {
    Fixture fixture; FileNode* file = fixture.add(L"\\file");
    FSP_FSCTL_FILE_INFO info{};
    auto resize = [&](UINT64 length, bool allocation = false) {
        return Interface::SetFileSize(&fixture.raw, file, length, allocation, &info);
    };
    auto check = [&](size_t begin, size_t end, byte value) {
        std::vector<byte> bytes(end - begin, 0xcc); ULONG count = 0;
        assert(Interface::Read(&fixture.raw, file, bytes.data(), begin, bytes.size(), &count) == STATUS_SUCCESS);
        assert(count == bytes.size() && std::all_of(bytes.begin(), bytes.end(), [=](byte b) { return b == value; }));
    };
    fixture.seed(file, 'A');
    assert(resize(4096) == STATUS_SUCCESS); assert(resize(8192) == STATUS_SUCCESS);
    check(0, 4096, 'A'); check(4096, 8192, 0);
    // Allocation truncation can retain the physical sector too.
    fixture.seed(file, 'A');
    assert(resize(4096, true) == STATUS_SUCCESS); assert(resize(8192) == STATUS_SUCCESS);
    check(0, 4096, 'A'); check(4096, 8192, 0);
    assert(resize(2 * FULL_SECTOR_SIZE) == STATUS_SUCCESS);
    std::memset(file->GetSectorNode().Sectors[0]->Bytes, 'B', FULL_SECTOR_SIZE);
    std::memset(file->GetSectorNode().Sectors[1]->Bytes, 'B', FULL_SECTOR_SIZE);
    assert(resize(FULL_SECTOR_SIZE - 7) == STATUS_SUCCESS);
    assert(resize(FULL_SECTOR_SIZE + 11) == STATUS_SUCCESS);
    check(FULL_SECTOR_SIZE - 7, FULL_SECTOR_SIZE + 11, 0);
    check(0, FULL_SECTOR_SIZE - 7, 'B');
    // Failed growth leaves EOF, allocation accounting and existing bytes unchanged.
    const UINT64 beforeSize = file->fileInfo.FileSize, beforeAllocation = file->fileInfo.AllocationSize;
    const UINT64 beforeSectors = fixture.fs.GetSectorManager().GetAllocatedSectors();
    virtualAllocationFailure = 1;
    assert(resize(4 * FULL_SECTOR_SIZE) == STATUS_INSUFFICIENT_RESOURCES);
    assert(file->fileInfo.FileSize == beforeSize && file->fileInfo.AllocationSize == beforeAllocation);
    assert(fixture.fs.GetSectorManager().GetAllocatedSectors() == beforeSectors);
    check(0, FULL_SECTOR_SIZE - 7, 'B');
    // A write after truncation must zero the gap even within a retained sector.
    assert(resize(4096) == STATUS_SUCCESS);
    byte payload[] = {'X', 'Y', 'Z'}; ULONG written = 0;
    assert(Interface::Write(&fixture.raw, file, payload, 8192, sizeof(payload), false, false, &written, &info) == STATUS_SUCCESS);
    assert(written == sizeof(payload)); check(4096, 8192, 0);
    byte readback[3]{}; ULONG count = 0;
    assert(Interface::Read(&fixture.raw, file, readback, 8192, 3, &count) == STATUS_SUCCESS);
    assert(std::memcmp(readback, payload, 3) == 0);
    // Extending from zero clears retained data after overwrite/truncate.
    assert(resize(0) == STATUS_SUCCESS); assert(resize(4096) == STATUS_SUCCESS); check(0, 4096, 0);
    std::cout << "EOF extension, retained allocation, sector crossings, write gaps and failed growth passed\n";
}
}
int main() {
    replacementAndFailures(false); replacementAndFailures(true);
    identityAndDirectories(); insertionAndCreateFailures(); resizeAndWrite();
    assert(DiagNodeUseAfterFree.load() == 0);
    std::cout << "native regressions passed (current production methods, portable OS stubs)\n";
}

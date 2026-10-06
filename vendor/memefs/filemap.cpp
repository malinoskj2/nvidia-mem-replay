#include "globalincludes.h"
#include "utils.h"
#include "memfs.h"
#include <unordered_set>

using namespace Memfs;

bool MemFs::IsCaseInsensitive() const {
	return this->fileMap.key_comp().CaseInsensitive;
}

std::refoptional<FileNode> MemFs::FindFile(const std::wstring_view& fileName) {
	MEMFS_LOOKUP_GUARD();

	const auto iter = this->fileMap.find(fileName);
	if (iter == this->fileMap.end()) {
		return {};
	}

	return *iter->second;
}

std::optional<FileNode*> MemFs::FindMainFromStream(const std::wstring_view& fileName) {
	MEMFS_LOOKUP_GUARD();

	const auto colonPos = std::ranges::find(fileName, L':');
	const std::wstring_view mainNameView = (colonPos != fileName.end())
		? std::wstring_view(fileName.data(), colonPos - fileName.begin())
		: fileName;

	const auto iter = this->fileMap.find(mainNameView);
	if (iter == this->fileMap.end()) {
		return {};
	}

	return iter->second;
}

std::pair<NTSTATUS, std::refoptional<FileNode>> MemFs::FindParent(const std::wstring_view& fileName) {
	MEMFS_LOOKUP_GUARD();

	const auto parentPath = Utils::PathSuffix(fileName).RemainPrefix;

	const auto iter = this->fileMap.find(parentPath);
	if (iter == this->fileMap.end()) {
		return {STATUS_OBJECT_PATH_NOT_FOUND, {}};
	}

	if (0 == (iter->second->fileInfo.FileAttributes & FILE_ATTRIBUTE_DIRECTORY)) {
		return {STATUS_NOT_A_DIRECTORY, {}};
	}

	return {STATUS_SUCCESS, *iter->second};
}

void MemFs::TouchParent(const FileNode& node) {
	if (node.fileName.empty() || node.fileName == L"\\") {
		return;
	}

	const auto [fst, snd] = this->FindParent(node.fileName);
	if (snd.has_value()) {
		FileNode& parent = snd.value();

		std::unique_lock lock(parent.nodeMutex);
		parent.fileInfo.LastAccessTime = parent.fileInfo.LastWriteTime = parent.fileInfo.ChangeTime = Utils::GetSystemTime();
	}
}

bool MemFs::HasChild(const FileNode& node) {
	MEMFS_LOOKUP_GUARD();

	const size_t nodeLen = node.fileName.length();
	const bool isRoot = (nodeLen == 1 && node.fileName[0] == L'\\');

	for (auto iter = this->fileMap.upper_bound(node.fileName); this->fileMap.end() != iter; ++iter) {
		const std::wstring& childName = iter->second->fileName;
		const size_t childLen = childName.length();

		// Skip named streams
		if (childName.find(L':') != std::wstring::npos) {
			continue;
		}

		// Check it is a direct child: prefix matches and no further separator after it
		bool isChild;
		if (isRoot) {
			isChild = childLen > 1 &&
			          childName.find(L'\\', 1) == std::wstring::npos;
		} else {
			isChild = childLen > nodeLen &&
			          childName[nodeLen] == L'\\' &&
			          childName.find(L'\\', nodeLen + 1) == std::wstring::npos;
		}

		if (isChild) {
			return true;
		}
		break; // map is sorted; first entry after upper_bound is the closest match
	}

	return false;
}

std::pair<NTSTATUS, FileNode*> MemFs::InsertNode(FileNode* node) {
	try {
		FileNode* resultNode;
		{
			std::unique_lock mapLock(this->fileMapMutex);
#if MEMFS_DIAGNOSTICS
			const WriterGuard writerGuard(*this);
#endif
			const auto [iter, success] = this->fileMap.emplace(node->fileName, node);
			resultNode = iter->second;
			if (!success) {
				this->diagInsertCollisions.fetch_add(1, std::memory_order_relaxed);
				FspDebugLog(__FUNCTION__ ": name collision - the name already belongs to another node\n");
				// A3: the name is taken by another node. Returning it under STATUS_SUCCESS would
				// hand the caller a handle to the wrong file and leak the node it wanted to insert.
				return {STATUS_OBJECT_NAME_COLLISION, resultNode};
			}

			iter->second->Reference();
#if MEMFS_DIAGNOSTICS
			this->diagMapEpoch.fetch_add(1);
#endif
		}

		// Publication succeeded. An advisory timestamp failure must not make the caller
		// destroy a node that the map now owns.
		try { this->TouchParent(*resultNode); } catch (...) {}
		return {STATUS_SUCCESS, resultNode};
	} catch (...) {
		return {STATUS_INSUFFICIENT_RESOURCES, node};
	}
}

std::pair<NTSTATUS, FileNode*> MemFs::InsertNode(FileNode&& node) {
	try {
		auto allocatedNode = std::make_unique<FileNode>(std::move(node));
		const auto [status, ptr] = this->InsertNode(allocatedNode.get());
		if (!NT_SUCCESS(status)) {
			return {status, nullptr};
		}

		allocatedNode.release(); // The successful insertion owns the map reference.
		return {status, ptr};
	} catch (const std::bad_alloc&) {
		return {STATUS_INSUFFICIENT_RESOURCES, nullptr};
	}
}

void MemFs::RemoveNode(FileNode& node, const bool reportDeletedSize) {
	{
		std::unique_lock mapLock(this->fileMapMutex);
#if MEMFS_DIAGNOSTICS
		const WriterGuard writerGuard(*this);
#endif

		const auto iter = this->fileMap.find(node.fileName);
		if (iter == this->fileMap.end()) {
			return;
		}

		// A1: the name may meanwhile belong to a *different* node — rename with
		// replaceIfExists, POSIX unlink with handles still open. Erasing it would unlink the
		// wrong entry and drop a reference this node never owned.
		if (iter->second != &node) {
			this->diagWrongNameRemovals.fetch_add(1, std::memory_order_relaxed);
			FspDebugLog(__FUNCTION__ ": name belongs to a different node - not unlinking\n");
			return;
		}

		this->fileMap.erase(iter);
#if MEMFS_DIAGNOSTICS
		this->diagMapEpoch.fetch_add(1, std::memory_order_relaxed);
#endif
	}

	this->TouchParent(node);
	node.Dereference();
}

namespace {
// Owns every allocation and node lock needed for a rename. Map iterators remain
// valid only while the caller holds the namespace lock through preparation/commit.
class RenamePlan {
	struct Change {
		FileNodeMap::iterator entry;
		std::wstring name;
	};
	std::vector<Change> changes;
	std::vector<FileNodeMap::iterator> replaced;
	FileNodeMap staged;
	std::vector<FileNode*> unlinked;
	std::vector<std::unique_lock<std::shared_mutex>> nodeLocks;

	NTSTATUS StageChanges(FileNodeMap& fileMap, FileNodeMap::iterator source,
		FileNode& node, std::wstring_view newFileName) {
		if (newFileName.size() >= MEMFS_MAX_PATH) {
			return STATUS_OBJECT_NAME_INVALID;
		}

		const size_t maxSuffixLength = MEMFS_MAX_PATH - newFileName.size();
		const size_t oldLength = node.fileName.size();
		for (auto iter = source; iter != fileMap.end(); ++iter) {
			FileNode* descendant = iter->second;
			if (!Utils::FileNameHasPrefix(descendant->fileName.c_str(), (int)descendant->fileName.size(), node.fileName.c_str(), (int)oldLength, fileMap.key_comp().CaseInsensitive)) {
				break;
			}

			const size_t suffixLength = descendant->fileName.size() - oldLength;
			if (suffixLength >= maxSuffixLength) {
				return STATUS_OBJECT_NAME_INVALID;
			}

			std::wstring name(newFileName);
			name.append(descendant->fileName, oldLength, suffixLength);
			if (!staged.emplace(name, descendant).second) {
				return STATUS_OBJECT_NAME_COLLISION;
			}
			changes.push_back({iter, std::move(name)});
		}

		return STATUS_SUCCESS;
	}

	void StageReplacement(FileNodeMap& fileMap, FileNodeMap::iterator destination) {
		replaced.push_back(destination);

		// Replacing a main file also unlinks all its streams, including streams
		// absent on the source. Open destination handles keep their own references.
		if (!destination->second->IsMainNode()) return;

		const std::wstring& destinationName = destination->second->fileName;
		for (auto iter = std::next(destination); iter != fileMap.end(); ++iter) {
			const std::wstring& name = iter->second->fileName;
			if (name.size() <= destinationName.size() || name[destinationName.size()] != L':' ||
				!Utils::FileNameHasPrefix(name.c_str(), (int)name.size(), destinationName.c_str(), (int)destinationName.size(), fileMap.key_comp().CaseInsensitive)) {
				break;
			}
			replaced.push_back(iter);
		}
	}

	NTSTATUS CheckCollisions(const FileNodeMap& fileMap) const {
		std::unordered_set<FileNode*> removedNodes;
		removedNodes.reserve(changes.size() + replaced.size());
		for (const auto& change : changes) removedNodes.insert(change.entry->second);
		for (const auto& entry : replaced) {
			// Moving a stream over its own main file would both replace and move the
			// same map entry. Reject overlapping trees before any iterator is erased.
			if (!removedNodes.insert(entry->second).second) return STATUS_ACCESS_DENIED;
		}

		for (const auto& [name, descendant] : staged) {
			const auto existing = fileMap.find(name);
			if (existing == fileMap.end()) continue;
			if (!removedNodes.contains(existing->second)) return STATUS_OBJECT_NAME_COLLISION;
		}

		return STATUS_SUCCESS;
	}

	void LockNodes() {
		unlinked.reserve(replaced.size());
		for (const auto& entry : replaced) unlinked.push_back(entry->second);

		nodeLocks.reserve(changes.size());
		for (const auto& change : changes) nodeLocks.emplace_back(change.entry->second->nodeMutex);
	}

public:
	explicit RenamePlan(const FileNodeMap& fileMap) : staged(fileMap.key_comp()) {}

	NTSTATUS Prepare(FileNodeMap& fileMap, FileNode& node,
		std::wstring_view newFileName, bool replaceIfExists) {
		const auto source = fileMap.find(node.fileName);
		if (source == fileMap.end() || source->second != &node) {
			return STATUS_OBJECT_NAME_NOT_FOUND;
		}

		if (node.fileName == L"\\" ||
			(newFileName.size() > node.fileName.size() &&
			 Utils::FileNameHasPrefix(newFileName.data(), (int)newFileName.size(), node.fileName.c_str(), (int)node.fileName.size(), fileMap.key_comp().CaseInsensitive))) {
			return STATUS_ACCESS_DENIED;
		}

		const auto destination = fileMap.find(newFileName);
		const bool replace = destination != fileMap.end() && destination->second != &node;
		if (replace) {
			if (!replaceIfExists) {
				return STATUS_OBJECT_NAME_COLLISION;
			}

			if (destination->second->fileInfo.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
				return STATUS_ACCESS_DENIED;
			}
		}

		const NTSTATUS stageStatus = StageChanges(fileMap, source, node, newFileName);
		if (!NT_SUCCESS(stageStatus)) return stageStatus;

		if (replace) StageReplacement(fileMap, destination);

		const NTSTATUS collisionStatus = CheckCollisions(fileMap);
		if (!NT_SUCCESS(collisionStatus)) return collisionStatus;

		LockNodes();
		return STATUS_SUCCESS;
	}

	// No allocations: transfer the staged map nodes and existing map references.
	void Commit(FileNodeMap& fileMap) {
		for (const auto& entry : replaced) fileMap.erase(entry);

		for (auto& change : changes) {
			change.entry->second->fileName.swap(change.name);
			fileMap.erase(change.entry);
		}

		while (!staged.empty()) fileMap.insert(staged.extract(staged.begin()));
	}

	void UnlockNodes() { nodeLocks.clear(); }

	void ReleaseReplacedNodes() {
		for (FileNode* replacedNode : unlinked) replacedNode->Dereference();
	}
};
}

NTSTATUS MemFs::RenameNode(FileNode& node, const std::wstring_view& newFileName, bool replaceIfExists) {
	// WinFsp's COARSE operation guard serializes namespace callbacks. Prepare every
	// allocation and lock before changing either namespace; commit transfers existing
	// map references with C++17 node handles, which do not allocate.
	try {
		std::unique_lock mapLock(this->fileMapMutex);
		RenamePlan plan(this->fileMap);
		const NTSTATUS status = plan.Prepare(this->fileMap, node, newFileName, replaceIfExists);
		if (!NT_SUCCESS(status)) return status;

		const auto [parentStatus, oldParent] = this->FindParent(node.fileName);

		{
#if MEMFS_DIAGNOSTICS
			const WriterGuard writerGuard(*this);
#endif
			plan.Commit(this->fileMap);
#if MEMFS_DIAGNOSTICS
			this->diagMapEpoch.fetch_add(1, std::memory_order_relaxed);
#endif
		}

		plan.UnlockNodes();
		mapLock.unlock();
		plan.ReleaseReplacedNodes();

		// Timestamp updates are advisory once the namespace transaction has committed.
		try {
			if (oldParent.has_value()) {
				FileNode& parent = oldParent.value();
				std::unique_lock parentLock(parent.nodeMutex);
				parent.fileInfo.LastAccessTime = parent.fileInfo.LastWriteTime = parent.fileInfo.ChangeTime = Utils::GetSystemTime();
			}

			this->TouchParent(node);
		} catch (...) {}

		return STATUS_SUCCESS;
	} catch (const std::bad_alloc&) {
		return STATUS_INSUFFICIENT_RESOURCES;
	}
}

void MemFs::DiagReport() {
	// wvsprintf-based loggers: keep the format specifiers to what they understand.
	const ULONG wrongName = (ULONG)this->diagWrongNameRemovals.load(std::memory_order_relaxed);
	const ULONG collisions = (ULONG)this->diagInsertCollisions.load(std::memory_order_relaxed);
	const ULONG unlinkedCleanups = (ULONG)this->diagCleanupDeleteUnlinked.load(std::memory_order_relaxed);

	FspDebugLog("memefs diagnostics: A1 wrong-name removals = %lu, A3 insert collisions = %lu, "
	            "A4 cleanup-delete on unlinked node = %lu\n", wrongName, collisions, unlinkedCleanups);
	FspServiceLog((wrongName | collisions | unlinkedCleanups) != 0 ? EVENTLOG_ERROR_TYPE : EVENTLOG_INFORMATION_TYPE,
	              (PWSTR)L"memefs diagnostics: A1 wrong-name removals = %lu, A3 insert collisions = %lu, "
	              L"A4 cleanup-delete on unlinked node = %lu", wrongName, collisions, unlinkedCleanups);

#if MEMFS_DIAGNOSTICS
	const ULONG lookups = (ULONG)this->diagLookups.load(std::memory_order_relaxed);
	const ULONG overlapped = (ULONG)this->diagOverlappedLookups.load(std::memory_order_relaxed);
	const ULONG epoch = (ULONG)this->diagMapEpoch.load(std::memory_order_relaxed);
	const ULONG useAfterFree = (ULONG)DiagNodeUseAfterFree.load(std::memory_order_relaxed);

	FspDebugLog("memefs diagnostics: D0 uses of a destroyed FileNode = %lu\n", useAfterFree);
	FspServiceLog(useAfterFree != 0 ? EVENTLOG_ERROR_TYPE : EVENTLOG_INFORMATION_TYPE,
	              (PWSTR)L"memefs diagnostics: D0 uses of a destroyed FileNode = %lu", useAfterFree);

	const ULONG maxInFlight = (ULONG)this->diagMaxLookupsInFlight.load();

	FspDebugLog("memefs diagnostics: D2 map writes = %lu, lookups = %lu, overlapped with a map write = %lu, "
	            "max concurrent lookups = %lu\n", epoch, lookups, overlapped, maxInFlight);
	FspServiceLog(overlapped != 0 ? EVENTLOG_WARNING_TYPE : EVENTLOG_INFORMATION_TYPE,
	              (PWSTR)L"memefs diagnostics: D2 map writes = %lu, lookups = %lu, overlapped = %lu",
	              epoch, lookups, overlapped);
#endif
}

std::vector<FileNode*> MemFs::EnumerateNamedStreams(const FileNode& node, const bool references) {
	std::vector<FileNode*> namedStreams;
	const int nodeLen = (int)node.fileName.length();

	for (auto iter = this->fileMap.upper_bound(node.fileName); this->fileMap.end() != iter; ++iter) {
		if (!Utils::FileNameHasPrefix(iter->second->fileName.c_str(), (int)iter->second->fileName.length(), node.fileName.c_str(), nodeLen, this->IsCaseInsensitive()))
			break;
		if (L':' != iter->second->fileName[nodeLen])
			break;

		if (references) {
			iter->second->Reference();
		}
		namedStreams.push_back(iter->second);
	}

	return namedStreams;
}

std::vector<FileNode*> MemFs::EnumerateDescendants(const FileNode& node, const bool references) {
	std::vector<FileNode*> descendants;
	const int nodeLen = (int)node.fileName.length();

	for (auto iter = this->fileMap.lower_bound(node.fileName); this->fileMap.end() != iter; ++iter) {
		if (!Utils::FileNameHasPrefix(iter->second->fileName.c_str(), (int)iter->second->fileName.length(), node.fileName.c_str(), nodeLen, this->IsCaseInsensitive()))
			break;

		if (references) {
			iter->second->Reference();
		}
		descendants.push_back(iter->second);
	}

	return descendants;
}

std::vector<FileNode*> MemFs::EnumerateDirChildren(const FileNode& node, const wchar_t* marker) {
	MEMFS_LOOKUP_GUARD();

	std::vector<FileNode*> children;
	FileNodeMap::iterator iter;
	const size_t nodeLen = node.fileName.length();
	const int nodeLenI = (int)nodeLen;
	const bool isRoot = (nodeLen == 1 && node.fileName[0] == L'\\');

	if (marker) {
		const bool needsSlash = !isRoot;
		std::wstring markerKey;
		markerKey.reserve(nodeLen + 1 + wcslen(marker));
		markerKey = node.fileName;
		if (needsSlash) markerKey += L'\\';
		markerKey += marker;

		iter = this->fileMap.upper_bound(markerKey);
	} else {
		iter = this->fileMap.upper_bound(node.fileName);
	}

	for (; this->fileMap.end() != iter; ++iter) {
		const std::wstring& childName = iter->second->fileName;
		const size_t childLen = childName.length();

		if (!Utils::FileNameHasPrefix(childName.c_str(), (int)childLen, node.fileName.c_str(), nodeLenI, this->IsCaseInsensitive()))
			break;

		// Direct child: next char after prefix is '\', no further '\' or ':' after it
		bool isDirectoryChild;
		if (isRoot) {
			isDirectoryChild = childLen > 1 &&
			                   childName.find(L'\\', 1) == std::wstring::npos &&
			                   childName.find(L':', 1) == std::wstring::npos;
		} else {
			isDirectoryChild = childLen > nodeLen &&
			                   childName[nodeLen] == L'\\' &&
			                   childName.find(L'\\', nodeLen + 1) == std::wstring::npos &&
			                   childName.find(L':', nodeLen + 1) == std::wstring::npos;
		}

		if (isDirectoryChild) {
			children.push_back(iter->second);
		}
	}

	return children;
}

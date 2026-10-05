#include "memfs-interface.h"

namespace Memfs::Interface {
	NTSTATUS GetFileInfo(FSP_FILE_SYSTEM* fileSystem, PVOID fileNode0, FSP_FSCTL_FILE_INFO* fileInfo) {
		const FileNode* fileNode = GetFileNode(fileNode0);
		std::shared_lock lock(fileNode->nodeMutex);
		fileNode->CopyFileInfo(fileInfo);
		return STATUS_SUCCESS;
	}

	NTSTATUS SetBasicInfo(FSP_FILE_SYSTEM* fileSystem,
	                      PVOID fileNode0, UINT32 fileAttributes,
	                      UINT64 creationTime, UINT64 lastAccessTime, UINT64 lastWriteTime, UINT64 changeTime, FSP_FSCTL_FILE_INFO* fileInfo) {
		FileNode* fileNode = GetFileNode(fileNode0);

		if (!fileNode->IsMainNode()) {
			fileNode = fileNode->GetMainNode();
		}

		std::unique_lock lock(fileNode->nodeMutex);

		if (INVALID_FILE_ATTRIBUTES != fileAttributes) {
			fileNode->fileInfo.FileAttributes = fileAttributes;
		}
		if (0 != creationTime) {
			fileNode->fileInfo.CreationTime = creationTime;
		}
		if (0 != lastAccessTime) {
			fileNode->fileInfo.LastAccessTime = lastAccessTime;
		}
		if (0 != lastWriteTime) {
			fileNode->fileInfo.LastWriteTime = lastWriteTime;
		}
		if (0 != changeTime) {
			fileNode->fileInfo.ChangeTime = changeTime;
		}

		fileNode->CopyFileInfo(fileInfo);
		return STATUS_SUCCESS;
	}

	NTSTATUS SetFileSize(FSP_FILE_SYSTEM* fileSystem,
	                     PVOID fileNode0, UINT64 newSize, BOOLEAN setAllocationSize,
	                     FSP_FSCTL_FILE_INFO* fileInfo) {
		FileNode* fileNode = GetFileNode(fileNode0);

		std::unique_lock lock(fileNode->nodeMutex);
		const NTSTATUS result = CompatSetFileSizeInternal(fileSystem, fileNode, newSize, setAllocationSize);
		if (!NT_SUCCESS(result)) {
			return result;
		}

		fileNode->CopyFileInfo(fileInfo);
		return STATUS_SUCCESS;
	}

	NTSTATUS CanDelete(FSP_FILE_SYSTEM* fileSystem, PVOID fileNode0, PWSTR fileName) {
		MemFs* memfs = GetMemFs(fileSystem);
		const FileNode* fileNode = GetFileNode(fileNode0);

		if (memfs->HasChild(*fileNode)) {
			return STATUS_DIRECTORY_NOT_EMPTY;
		}

		return STATUS_SUCCESS;
	}

	NTSTATUS Rename(FSP_FILE_SYSTEM* fileSystem, PVOID fileNode0,
	                PWSTR fileName, PWSTR newFileName, BOOLEAN replaceIfExists) {
		MemFs* memfs = GetMemFs(fileSystem);
		FileNode* fileNode = GetFileNode(fileNode0);

		return memfs->RenameNode(*fileNode, newFileName, replaceIfExists);
	}
}

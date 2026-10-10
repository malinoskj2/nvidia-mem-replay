#include "globalincludes.h"
#include "exceptions.h"

namespace Memfs {
	CreateException::CreateException(const NTSTATUS status)
		: status(status), message("Create Exception with NTStatus: " + std::to_string(status)) {
	}

	char const* CreateException::what() const noexcept {
		// The message lives as long as the exception; a temporary's c_str() would dangle.
		return this->message.c_str();
	}

	NTSTATUS CreateException::Which() const {
		return this->status;
	}

	char const* FileNameTooLongException::what() const noexcept {
		return "The file name is too long.";
	}
}

#pragma once

#include "globalincludes.h"

namespace Memfs {
	class CreateException final : public std::exception {
	public:
		explicit CreateException(const NTSTATUS status);

		[[nodiscard]] char const* what() const noexcept override;
		[[nodiscard]] NTSTATUS Which() const;

	private:
		NTSTATUS status;
		std::string message;
	};

	class FileNameTooLongException final : public std::exception {
		[[nodiscard]] char const* what() const noexcept override;
	};
}

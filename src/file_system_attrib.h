#pragma once

#include "variant.h"
#include "shortcuts.h"

#include <iosfwd>

namespace ouisync {

struct FileSystemDirAttrib {};
struct FileSystemFileAttrib { size_t size; };

using FileSystemAttrib = variant<FileSystemDirAttrib, FileSystemFileAttrib>;

std::ostream& operator<<(std::ostream& os, FileSystemFileAttrib);
std::ostream& operator<<(std::ostream& os, FileSystemDirAttrib);
std::ostream& operator<<(std::ostream& os, FileSystemAttrib);

} // namespace

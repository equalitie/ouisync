#include "file_system_attrib.h"
#include <ostream>

std::ostream& ouisync::operator<<(std::ostream& os, FileSystemFileAttrib) {
    return os << "FileSystemFileAttrib";
}

std::ostream& ouisync::operator<<(std::ostream& os, FileSystemDirAttrib) {
    return os << "FileSystemDirAttrib";
}

std::ostream& ouisync::operator<<(std::ostream& os, FileSystemAttrib a) {
    apply(a, [&os] (auto& b) { os << b; });
    return os;
}

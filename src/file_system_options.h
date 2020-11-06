#pragma once

#include "shortcuts.h"
#include <boost/filesystem/operations.hpp>

namespace ouisync {

class FileSystemOptions {
public:
    FileSystemOptions(fs::path basedir) :
        _basedir(std::move(basedir))
    {
        fs::create_directories(branchdir());
        fs::create_directories(objectdir());
    }

    const fs::path& basedir() const { return _basedir; }
    fs::path branchdir() const { return _basedir / "branches"; }
    fs::path objectdir() const { return _basedir / "objects"; }

    fs::path user_id_file_path() const {
        return _basedir / "user_id";
    }

private:
    fs::path _basedir;
};

} // namespace

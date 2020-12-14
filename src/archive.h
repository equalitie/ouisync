#pragma once

#include "shortcuts.h"
#include <boost/filesystem/fstream.hpp>
#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>

namespace ouisync {

// Using subclassing instead of typedefs to be able to use forward declaration.

class InputArchive : public boost::archive::binary_iarchive
{
    public:
    using boost::archive::binary_iarchive::binary_iarchive;
};

class OutputArchive : public boost::archive::binary_oarchive
{
    public:
    using boost::archive::binary_oarchive::binary_oarchive;
};

namespace archive {
    template<class Obj>
    void load(const fs::path& path, Obj& out)
    {
        fs::ifstream ifs(path, fs::ifstream::binary);
        if (!ifs.is_open()) {
            throw std::runtime_error("archive::load: Failed to open object");
        }
        InputArchive ia(ifs);
        ia >> out;
    }

    template<class Obj>
    void store(const fs::path& path, const Obj& obj)
    {
        fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);
        assert(ofs.is_open());
        if (!ofs.is_open()) throw std::runtime_error("Failed to store object");
        OutputArchive oa(ofs);
        oa << obj;
    }
}

} // namespace

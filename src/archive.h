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

    //----------------------------------------------------------------

    namespace detail {
        template<class Obj>
        void load(InputArchive& o, Obj& obj)
        {
            o >> obj;
        }
        template<class Obj, class... OtherObjs>
        void load(InputArchive& o, Obj& obj, OtherObjs&... other_objs)
        {
            o >> obj;
            load(o, other_objs...);
        }
        template<class Obj>
        void store(OutputArchive& o, const Obj& obj)
        {
            o << obj;
        }
        template<class Obj, class... OtherObjs>
        void store(OutputArchive& o, const Obj& obj, const OtherObjs&... other_objs)
        {
            o << obj;
            store(o, other_objs...);
        }
    }

    //--- load -------------------------------------------------------

    template<class Obj, class... OtherObjs>
    void load(std::istream& s, Obj& obj, OtherObjs&... other_objs)
    {
        InputArchive a(s);
        detail::load(a, obj, other_objs...);
    }

    template<class Obj, class... OtherObjs>
    void load(const fs::path& path, Obj& obj, OtherObjs&... other_objs)
    {
        fs::ifstream ifs(path, fs::ifstream::binary);
        if (!ifs.is_open()) {
            throw std::runtime_error("archive::load: Failed to open object");
        }
        load(ifs, obj, other_objs...);
    }

    //--- store ------------------------------------------------------

    template<class Obj, class... OtherObjs>
    void store(std::ostream& s, const Obj& obj, const OtherObjs&... other_objs)
    {
        OutputArchive a(s);
        detail::store(a, obj, other_objs...);
    }

    template<class Obj, class... OtherObjs>
    void store(const fs::path& path, const Obj& obj, const OtherObjs&... other_objs)
    {
        fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);
        assert(ofs.is_open());
        if (!ofs.is_open()) throw std::runtime_error("Failed to store object");
        store(ofs, obj, other_objs...);
    }

    //----------------------------------------------------------------
}

} // namespace

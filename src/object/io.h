#pragma once

#include "id.h"
#include "tagged.h"
#include "path.h"

#include <boost/archive/text_iarchive.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::io {

// --- store ---------------------------------------------------------

namespace detail {
    template<class O>
    inline
    void store_at(const fs::path& path, const O& object) {
        // XXX: if this probes every single directory in path, then it might be
        // slow and in such case we could instead try to create only the last 2.
        fs::create_directories(path.parent_path());
        fs::ofstream ofs(path, std::fstream::out | std::fstream::binary | std::fstream::trunc);
        if (!ofs.is_open()) throw std::runtime_error("Failed to store object");
        boost::archive::text_oarchive oa(ofs);
        tagged::Save<O> save{object};
        oa << save;
    }
}

/*
 * Store a single and flat (no subnodes) object to disk.  The type O may be any
 * object that has GetTag<O> specialization (in tag.h) as well as standard
 * serialization methods.
 * Returns Id of the stored object.
 */
template<class O>
inline
Id store(const fs::path& objdir, const O& object) {
    auto id = object.calculate_id();
    auto path = objdir / path::from_id(id);
    detail::store_at(path, object);
    return id;
}

/*
 * As above, but don't replace block if already exists
 */
template<class O>
inline
Opt<Id> maybe_store(const fs::path& objdir, const O& object) {
    auto id = object.calculate_id();
    auto path = objdir / path::from_id(id);
    if (fs::exists(path)) return boost::none;
    detail::store_at(path, object);
    return id;
}

Id store(const fs::path& objdir, Id root_tree, const fs::path& objpath, const Block&);

Opt<Id> maybe_store(const fs::path& objdir, Id root_tree, const fs::path& objpath, const Block&);


// --- load ----------------------------------------------------------

template<class O>
inline
O load(const fs::path& path) {
    fs::ifstream ifs(path, fs::ifstream::binary);
    if (!ifs.is_open())
        throw std::runtime_error("Failed to open object");
    boost::archive::text_iarchive ia(ifs);
    O result;
    tagged::Load<O> loader{result};
    ia >> loader;
    return result;
}

//------------------------------------
template<class O>
inline
O load(const fs::path& objdir, const Id& id) {
    return load<O>(objdir / path::from_id(id));
}

template<class O0, class O1, class ... Os> // Two or more
inline
variant<O0, O1, Os...> load(const fs::path& objdir, const Id& id) {
    return load<variant<O0, O1, Os...>>(objdir / path::from_id(id));
}

template<class O0, class O1, class ... Os> // Two or more
inline
variant<O0, O1, Os...> load(const fs::path& path) {
    return load<variant<O0, O1, Os...>>(path);
}

//------------------------------------
template<class O>
inline
Opt<O> maybe_load(const fs::path& objdir, const Id& id) {
    auto p = objdir / path::from_id(id);
    if (!fs::exists(p)) return boost::none;
    return load<O>(p);
}

template<class O0, class O1, class ... Os> // Two or more
inline
Opt<variant<O0, O1, Os...>> maybe_load(const fs::path& objdir, const Id& id) {
    return maybe_load<variant<O0, O1, Os...>>(objdir, id);
}

//------------------------------------
Block load(const fs::path& objdir, const Id& root_id, const fs::path& objpath);

// --- remove --------------------------------------------------------

bool remove(const fs::path& objdir, const Id& id);

// -------------------------------------------------------------------

} // namespace

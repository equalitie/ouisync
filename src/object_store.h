#pragma once

#include "object_id.h"
#include "archive.h"
#include "shortcuts.h"
#include "object_tag.h"

#include <boost/filesystem/path.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp> // create_directories
#include <boost/format.hpp>

namespace ouisync {

// -------------------------------------------------------------------
namespace detail {

std::exception bad_tag_exception(ObjectTag requested, ObjectTag parsed);

template<class T, class... Ts> struct LoadVariant {
    template<class Variant, class Archive>
    static void load(ObjectTag tag, Variant& result, Archive& ar) {
        if (tag == T::tag) {
            T obj;
            ar & obj;
            result = std::move(obj);
            return;
        }
        LoadVariant<Ts...>::load(tag, result, ar);
    }
};
template<class T> struct LoadVariant<T> {
    template<class Variant, class Archive>
    static void load(ObjectTag tag, Variant& result, Archive& ar) {
        if (tag == T::tag) {
            T obj;
            ar & obj;
            result = std::move(obj);
            return;
        }
        throw boost::archive::archive_exception(
                boost::archive::archive_exception::unregistered_class);
    }
};

template<class Obj>
struct SaveWithTag {
    const Obj& obj;

    template<class Archive>
    void save(Archive& ar, const unsigned int version) const {
        ar & Obj::tag;
        ar & obj;
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

template<class Obj>
struct LoadWithTag {
    Obj& obj;

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        ObjectTag tag;
        ar & tag;

        if (tag != Obj::tag) {
            throw bad_tag_exception(Obj::tag, tag);
        }

        ar & obj;
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()

};


template<class... Ts>
struct LoadWithTag<boost::variant<Ts...>> {
    using Variant = boost::variant<Ts...>;
    Variant& var;

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        ObjectTag tag;
        ar & tag;
        detail::LoadVariant<Ts...>::load(tag, var, ar);
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

} // detail namespace
// -------------------------------------------------------------------

class ObjectStore {
public:
    ObjectStore(fs::path object_dir);

    template<class O>
    ObjectId store(const O& object);

    template<class O>
    std::pair<ObjectId, bool> store_(const O& object);

    template<class O> O load(const ObjectId& id);
    
    template<class O0, class O1, class ... Os> // Two or more
    variant<O0, O1, Os...> load(const ObjectId& id);
    
    template<class O> static O load(const fs::path& path);

    template<class O0, class O1, class ... Os> // Two or more
    variant<O0, O1, Os...> static load(const fs::path& path);

    template<class O>
    Opt<O> maybe_load(const ObjectId& id);
    
    template<class O0, class O1, class ... Os> // Two or more
    Opt<variant<O0, O1, Os...>> maybe_load(const ObjectId& id);

    /*
     * Remove a single object. Note that this keeps children if it's a Tree.
     */
    bool remove(const ObjectId& id);
    
    bool exists(const ObjectId& id) const;
    
    bool is_complete(const ObjectId& id);

private:
    template<class O>
    void store_at(const fs::path& path, const O& object);

    fs::path id_to_path(const ObjectId&) const noexcept;

private:
    fs::path _objdir;
};

template<class O>
inline
void ObjectStore::store_at(const fs::path& path, const O& object) {
    // XXX: if this probes every single directory in path, then it might be
    // slow and in such case we could instead try to create only the last 2.
    fs::create_directories(path.parent_path());
    fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);
    assert(ofs.is_open());
    if (!ofs.is_open()) throw std::runtime_error("Failed to store object");
    OutputArchive oa(ofs);
    detail::SaveWithTag<O> save{object};
    oa << save;
}

/*
 * Store a single and flat (no subnodes) object to disk.  The type O may be any
 * object that has GetTag<O> specialization (in tag.h) as well as standard
 * serialization methods.
 * Returns Id of the stored object.
 */
template<class O>
inline
ObjectId ObjectStore::store(const O& object) {
    auto id = object.calculate_id();
    auto path = _objdir / id_to_path(id);
    store_at(path, object);
    return id;
}

/*
 * As above, but don't replace block if already exists
 */
template<class O>
inline
std::pair<ObjectId, bool> ObjectStore::store_(const O& object) {
    auto id = object.calculate_id();
    auto path = _objdir / id_to_path(id);
    if (fs::exists(path)) return {id, false};
    store_at(path, object);
    return {id, true};
}

// --- load ----------------------------------------------------------

template<class O>
inline
O ObjectStore::load(const fs::path& path) {
    fs::ifstream ifs(path, fs::ifstream::binary);
    if (!ifs.is_open()) {
        throw std::runtime_error(str(boost::format("Failed to open object %1%") % path));
    }
    InputArchive ia(ifs);
    O result;
    detail::LoadWithTag<O> loader{result};
    ia >> loader;
    return result;
}

//------------------------------------
template<class O>
inline
O ObjectStore::load(const ObjectId& id) {
    return load<O>(_objdir / id_to_path(id));
}

template<class O0, class O1, class ... Os> // Two or more
inline
variant<O0, O1, Os...> ObjectStore::load(const ObjectId& id) {
    return load<variant<O0, O1, Os...>>(_objdir / id_to_path(id));
}

template<class O0, class O1, class ... Os> // Two or more
inline
variant<O0, O1, Os...> ObjectStore::load(const fs::path& path) {
    return load<variant<O0, O1, Os...>>(path);
}

//------------------------------------
template<class O>
inline
Opt<O> ObjectStore::maybe_load(const ObjectId& id) {
    auto p = _objdir / id_to_path(id);
    if (!fs::exists(p)) return boost::none;
    return load<O>(p);
}

template<class O0, class O1, class ... Os> // Two or more
inline
Opt<variant<O0, O1, Os...>> ObjectStore::maybe_load(const ObjectId& id) {
    auto p = _objdir / id_to_path(id);
    if (!fs::exists(p)) return boost::none;
    return load<O0, O1, Os...>(p);
}

} // namespace

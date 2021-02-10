#include "object_store.h"
#include "object/blob.h"
#include "directory.h"
#include "hex.h"

#include "variant.h"

#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

using namespace ouisync;
using namespace std;
using object::Blob;

ObjectStore::ObjectStore(fs::path object_dir) :
    _objdir(move(object_dir))
{
    assert(fs::exists(_objdir));
    assert(fs::is_directory(_objdir));
}

bool ObjectStore::remove(const ObjectId& id) {
    sys::error_code ec;
    fs::remove(_objdir/id_to_path(id), ec);
    return !ec;
}

bool ObjectStore::exists(const ObjectId& id) const {
    return fs::exists(_objdir/id_to_path(id));
}

bool ObjectStore::is_complete(const ObjectId& id) {
    if (!exists(id)) return false;

    auto v = load<Blob::Nothing, Directory>(id);

    return apply(v,
        [&] (const Blob::Nothing&) { return true; },
        [&] (const Directory&) {
            assert("TODO" && 0);
            //for (auto& [_, id] : tree) {
            //    if (!is_complete(id)) return false;
            //}
            return true;
        });
}

fs::path ObjectStore::id_to_path(const ObjectId& id) const noexcept
{
    auto hex = to_hex<char>(id);
    string_view hex_sv{hex.data(), hex.size()};

    static const size_t prefix_size = 3;

    auto prefix = hex_sv.substr(0, prefix_size);
    auto rest   = hex_sv.substr(prefix_size, hex_sv.size() - prefix_size);

    return fs::path(prefix.begin(), prefix.end()) / fs::path(rest.begin(), rest.end());
}

//Opt<ObjectId> ObjectStore::path_to_id(const fs::path& ps) const noexcept
//{
//    std::array<char, ObjectId::size * 2> hex;
//
//    auto i = hex.begin();
//
//    for (auto& p : ps) {
//        for (auto c : p.native()) {
//            if (i == hex.end()) return boost::none;
//            *i++ = c;
//        }
//    }
//
//    if (i != hex.end()) return boost::none;
//
//    auto opt_bin = from_hex<uint8_t>(hex);
//    if (!opt_bin) return boost::none;
//    return ObjectId{*opt_bin};
//}

std::exception ouisync::detail::bad_tag_exception(ObjectTag requested, ObjectTag parsed)
{
    std::stringstream ss;
    ss << "Object not of the requested type "
       << "(requested:" << requested << "; parsed:" << parsed << ")";
    return std::runtime_error(ss.str());
}


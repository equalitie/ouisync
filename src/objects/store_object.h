#pragma once

#include "tagged.h"

#include <boost/archive/text_oarchive.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::objects {

template<class O>
inline
Opt<Sha256::Digest> store_object(const fs::path& root, const O& object) {
    auto digest = object.calculate_digest();
    auto path = root / Object::path_from_digest(digest);
    // XXX: if this probes every single directory in path, then it might be
    // slow and in such case we could instead try to create only the last 2.
    fs::create_directories(path.parent_path());
    fs::ofstream ofs(path, std::fstream::out | std::fstream::binary | std::fstream::trunc);
    if (!ofs.is_open()) return boost::none;
    boost::archive::text_oarchive oa(ofs);
    tagged::Save<O> save{object};
    oa << save;
    return digest;
}

} // namespaces

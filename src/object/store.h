#pragma once

#include "id.h"
#include "tagged.h"
#include "path.h"

#include <boost/optional.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object {

template<class O>
inline
Id store(const fs::path& root, const O& object) {
    auto id = object.calculate_id();
    auto path = root / path::from_id(id);
    // XXX: if this probes every single directory in path, then it might be
    // slow and in such case we could instead try to create only the last 2.
    fs::create_directories(path.parent_path());
    fs::ofstream ofs(path, std::fstream::out | std::fstream::binary | std::fstream::trunc);
    if (!ofs.is_open()) throw std::runtime_error("Failed to store object");
    boost::archive::text_oarchive oa(ofs);
    tagged::Save<O> save{object};
    oa << save;
    return id;
}

} // namespaces

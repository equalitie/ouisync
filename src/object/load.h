#pragma once

#include "id.h"
#include "tagged.h"
#include "object.h"
#include "path.h"

#include <boost/optional.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object {

template<class O>
inline
O load(const fs::path& root, const Id& id) {
    fs::ifstream ifs(root / path::from_id(id), fs::ifstream::binary);
    if (!ifs.is_open())
        throw std::runtime_error("Failed to open object");
    boost::archive::text_iarchive ia(ifs);
    O result;
    tagged::Load<O> loader{result};
    ia >> loader;
    return result;
}

} // namespaces

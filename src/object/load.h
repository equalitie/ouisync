#pragma once

#include "id.h"
#include "tagged.h"
#include "path.h"

#include <boost/archive/text_iarchive.hpp>
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object {

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

} // namespaces

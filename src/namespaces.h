#pragma once

#include <boost/utility/string_view_fwd.hpp>
#include <boost/optional/optional_fwd.hpp>

namespace boost {
    template<class T> class optional;
    namespace filesystem {}
    namespace uuids { struct uuid; }
}

namespace ouisync {

namespace fs = boost::filesystem;
template<class T> using Opt = boost::optional<T>;
using boost::string_view;
using Uuid = boost::uuids::uuid;

} // namespace

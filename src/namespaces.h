#pragma once

#include <boost/utility/string_view_fwd.hpp>
#include <boost/optional/optional_fwd.hpp>

namespace boost {
    template<class T> class optional;
    namespace filesystem {
        class path;
    }
    namespace uuids { struct uuid; }
    namespace system {
        class error_code;
    }
}

namespace ouisync {

namespace fs = boost::filesystem;
namespace sys = boost::system;
template<class T> using Opt = boost::optional<T>;
using boost::string_view;
using Uuid = boost::uuids::uuid;

} // namespace

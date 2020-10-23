#pragma once

#include <boost/utility/string_view_fwd.hpp>
#include <boost/optional/optional_fwd.hpp>
#include <boost/variant/variant_fwd.hpp>

namespace boost {
    template<class T> class optional;
    namespace filesystem {
        class path;
    }
    namespace system {
        class error_code;
    }
    namespace asio {
    }
}

namespace ouisync {

namespace fs = boost::filesystem;
namespace sys = boost::system;
namespace net = boost::asio;

template<class T> using Opt = boost::optional<T>;
using boost::string_view;
using boost::variant;

} // namespace

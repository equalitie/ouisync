#pragma once

#include "shortcuts.h"

namespace ouisync {

inline void throw_error(const sys::error_code& ec)
{
    namespace errc = boost::system::errc;
    assert(ec);
    if (!ec) throw sys::system_error(make_error_code(errc::no_message));
    throw sys::system_error(ec);
}

inline void throw_errno(int errno_)
{
    namespace errc = boost::system::errc;
    auto ec = make_error_code(static_cast<errc::errc_t>(errno_));
    throw_error(ec);
}

inline void throw_errno()
{
    throw_errno(errno);
}

} // namespace

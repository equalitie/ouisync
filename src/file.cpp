#include "file.h"
#include "error.h"

#include <boost/asio/use_awaitable.hpp>
#include <boost/asio/read.hpp>
#include <boost/asio/write.hpp>
#include <boost/filesystem/path.hpp>
#include <iostream>

using namespace ouisync;

namespace errc = boost::system::errc;
namespace posix = net::posix;

void File::seek(size_t pos)
{
    if (::lseek(_f.native_handle(), pos, SEEK_SET) == -1) {
        throw_errno();
    }
}

size_t File::current_position()
{
    off_t offset = ::lseek(_f.native_handle(), 0, SEEK_CUR);

    if (offset == -1) {
        throw_errno();
    }

    return offset;
}

size_t File::size()
{
    auto start_pos = current_position();

    if (::lseek(_f.native_handle(), 0, SEEK_END) == -1) {
        throw_errno();
    }

    auto end = current_position();
    seek(start_pos);

    return end;
}

File::File(const executor_type& ex, int file_handle) :
    _f(ex, file_handle)
{
}

/* static */
File File::truncate_or_create(const executor_type& exec, const fs::path& p)
{
    int file = ::open(p.c_str(), O_RDWR | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR);
    if (file == -1) throw_errno();
    return File(exec, file);
}

/* static */
File File::open_readonly(const executor_type& exec, const fs::path& p)
{
    int file = ::open(p.c_str(), O_RDONLY);
    if (file == -1) throw_errno();
    return File(exec, file);
}

net::awaitable<size_t> File::read(net::mutable_buffer b, Cancel cancel)
try {
    auto con = _scoped_cancel.connect(cancel, [&] { _f.close(); });
    size_t size = co_await net::async_read(_f, b, net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);
    co_return size;
}
catch (...) {
    if (cancel) throw_error(net::error::operation_aborted);
    throw;
}

net::awaitable<void> File::write(net::const_buffer b, Cancel cancel)
try {
    auto con = _scoped_cancel.connect(cancel, [&] { _f.close(); });
    co_await net::async_write(_f, b, net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);
}
catch (...) {
    if (cancel) throw_error(net::error::operation_aborted);
    throw;
}

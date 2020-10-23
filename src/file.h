#pragma once

#include "cancel.h"
#include "shortcuts.h"

#include <boost/asio/posix/stream_descriptor.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

class File {
public:
    using executor_type = net::posix::stream_descriptor::executor_type;

public:
    // Note: this does _not_ attempt create the directories in the path.
    static File truncate_or_create(net::io_context&, const fs::path&);
    static File truncate_or_create(const executor_type&, const fs::path&);

    static File open_readonly(net::io_context&, const fs::path&);
    static File open_readonly(const executor_type&, const fs::path&);

    void seek(size_t position);
    size_t current_position();
    size_t size();

    net::awaitable<size_t> read(net::mutable_buffer, Cancel);
    net::awaitable<void>   write(net::const_buffer, Cancel);

private:
    File(const executor_type&, int file_handle);

private:
    net::posix::stream_descriptor _f;
    ScopedCancel _scoped_cancel;
};

inline
File File::truncate_or_create(net::io_context& ioc, const fs::path& p) {
    return File::truncate_or_create(ioc.get_executor(), p);
}

inline
File File::open_readonly(net::io_context& ioc, const fs::path& p) {
    return File::open_readonly(ioc.get_executor(), p);
}

} // namespace

#pragma once

#include "shortcuts.h"

#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/executor_work_guard.hpp>
#include <boost/filesystem/path.hpp>
#include <thread>

namespace ouisync {

class Fuse {
private:
    struct Impl;

public:
    using executor_type = net::any_io_executor;

public:
    Fuse(executor_type ex, fs::path mountdir);

    void exit_loop();

    ~Fuse();

private:
    std::unique_ptr<Impl> _impl;

};

} // namespace

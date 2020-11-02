#pragma once

#include "shortcuts.h"
#include "cancel.h"

#define FUSE_USE_VERSION 34
#include <fuse_lowlevel.h>

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

    void finish();

    ~Fuse();

private:
    void run_loop();

    static void fuse_ll_lookup(fuse_req_t req, fuse_ino_t parent, const char *name);
    static void fuse_ll_getattr(fuse_req_t req, fuse_ino_t ino, struct fuse_file_info *fi);
    static void fuse_ll_read(fuse_req_t req, fuse_ino_t ino, size_t size, off_t off, struct fuse_file_info *fi);
    static void fuse_ll_readdir(fuse_req_t req, fuse_ino_t ino, size_t size, off_t off, struct fuse_file_info *fi);
    static void fuse_ll_open(fuse_req_t req, fuse_ino_t ino, struct fuse_file_info *fi);

private:
    executor_type _ex;
    const fs::path _mountdir;
    std::thread _thread;
    net::executor_work_guard<executor_type> _work;
    std::mutex _mutex;
    ScopedCancel _scoped_cancel;

    fuse_session *_fuse_session = nullptr;
    fuse_chan *_fuse_channel = nullptr;
    fuse_args _fuse_args;
};

} // namespace

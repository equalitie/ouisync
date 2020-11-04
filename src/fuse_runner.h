#pragma once

#include "shortcuts.h"
#include "cancel.h"
#include "result.h"

#define FUSE_USE_VERSION 26
#include <fuse.h>

#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/executor_work_guard.hpp>
#include <boost/filesystem/path.hpp>
#include <thread>

namespace ouisync {

class FileSystem;

class FuseRunner {
private:
    struct Impl;

public:
    using executor_type = net::any_io_executor;

public:
    FuseRunner(FileSystem&, fs::path mountdir);

    void finish();

    ~FuseRunner();

private:
    void run_loop();

    static void* _fuse_init(struct fuse_conn_info*);
    static int _fuse_getattr(const char* path, struct stat*);
    static int _fuse_readdir(const char* path, void *buf, fuse_fill_dir_t filler, off_t, fuse_file_info*);
    static int _fuse_open(const char* path, fuse_file_info*);
    static int _fuse_read(const char* path, char *buf, size_t size, off_t offset, fuse_file_info *fi);
    static int _fuse_truncate(const char* path, off_t);
    static int _fuse_mknod(const char* path, mode_t, dev_t);
    static int _fuse_utime(const char* path, utimbuf*);
    static int _fuse_unlink(const char* path);

    template<class F,
        class R = typename std::decay_t<std::result_of_t<F(FileSystem&)>>::value_type
        >
    static Result<R> query_fs(F&& f);

private:
    FileSystem& _fs;
    const fs::path _mountdir;
    std::thread _thread;
    net::executor_work_guard<executor_type> _work;
    std::mutex _mutex;
    ScopedCancel _scoped_cancel;

    fuse_chan *_fuse_channel = nullptr;
    fuse_args _fuse_args;
    struct fuse* _fuse;
};

} // namespace

#include "fuse_runner.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>
#include <iostream>

#include "defer.h"

#include <boost/asio/signal_set.hpp>

using namespace ouisync;

const char* filename = "hello";
const char* contents = "hello world";

FuseRunner::FuseRunner(executor_type ex, fs::path mountdir) :
    _ex(ex),
    _mountdir(std::move(mountdir)),
    _work(net::make_work_guard(_ex))
{
    const struct fuse_operations ouisync_fuse_oper = {
        .getattr        = ouisync_fuse_getattr,
        .open           = ouisync_fuse_open,
        .read           = ouisync_fuse_read,
        .readdir        = ouisync_fuse_readdir,
        .init           = ouisync_fuse_init,
    };

    static const char* argv[] = { "ouisync" };

    _fuse_args = FUSE_ARGS_INIT(1, (char**) argv);
    auto free_args_on_exit = defer([&] { fuse_opt_free_args(&_fuse_args); });

    _fuse_channel = fuse_mount(_mountdir.c_str(), &_fuse_args);
    if (!_fuse_channel)
        throw std::runtime_error("FUSE: Failed to mount");

    _fuse = fuse_new(_fuse_channel, &_fuse_args, &ouisync_fuse_oper, sizeof(ouisync_fuse_oper), nullptr);
    if (!_fuse)
        throw std::runtime_error("FUSE: failed in fuse_new");

    _thread = std::thread([this] { run_loop(); });
}

/* static */
void* FuseRunner::ouisync_fuse_init(struct fuse_conn_info *conn)
{
    (void) conn;
    return nullptr;
}

/* static */
int FuseRunner::ouisync_fuse_getattr(const char *path, struct stat *stbuf)
{
    int res = 0;
    memset(stbuf, 0, sizeof(struct stat));
    if (strcmp(path, "/") == 0) {
        stbuf->st_mode = S_IFDIR | 0755;
        stbuf->st_nlink = 2;
    } else if (strcmp(path+1, filename) == 0) {
        stbuf->st_mode = S_IFREG | 0444;
        stbuf->st_nlink = 1;
        stbuf->st_size = strlen(contents);
    } else {
        res = -ENOENT;
    }
    return res;
}

/* static */
int FuseRunner::ouisync_fuse_readdir(const char *path, void *buf, fuse_fill_dir_t filler,
                         off_t offset, struct fuse_file_info *fi)
{
    (void) offset;
    (void) fi;
    if (strcmp(path, "/") != 0)
        return -ENOENT;
    filler(buf, ".", NULL, 0);
    filler(buf, "..", NULL, 0);
    filler(buf, filename, NULL, 0);
    return 0;
}

/* static */
int FuseRunner::ouisync_fuse_open(const char *path, struct fuse_file_info *fi)
{
    if (strcmp(path+1, filename) != 0)
        return -ENOENT;
    if ((fi->flags & O_ACCMODE) != O_RDONLY)
        return -EACCES;
    return 0;
}

/* static */
int FuseRunner::ouisync_fuse_read(const char *path, char *buf, size_t size, off_t offset,
                      struct fuse_file_info*)
{
    if(strcmp(path+1, filename) != 0) return -ENOENT;
    size_t len = strlen(contents);
    if (offset < len) {
        if (offset + size > len) size = len - offset;
        memcpy(buf, contents + offset, size);
    } else {
        size = 0;
    }
    return size;
}

void FuseRunner::run_loop()
{
    int err = fuse_loop(_fuse);
    if (err) throw std::runtime_error("FUSE: Session loop returned error");
}

void FuseRunner::finish()
{
    if (!_fuse_channel) return;
    auto c = _fuse_channel;
    _fuse_channel = nullptr;
    fuse_unmount(_mountdir.c_str(), c);
    _work.reset();
}

FuseRunner::~FuseRunner() {
    finish();
    _thread.join();
    if (_fuse) fuse_destroy(_fuse);
}

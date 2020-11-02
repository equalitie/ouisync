#include "fuse.h"

#define FUSE_USE_VERSION 34

#include <fuse_lowlevel.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>
#include <iostream>

#include "defer.h"
#include "cancel.h"

#include <boost/asio/signal_set.hpp>

using namespace ouisync;

static const char *hello_str = "Hello World!\n";
static const char *hello_name = "hello";

static int hello_stat(fuse_ino_t ino, struct stat *stbuf)
{
	stbuf->st_ino = ino;
	switch (ino) {
	case 1:
		stbuf->st_mode = S_IFDIR | 0755;
		stbuf->st_nlink = 1;
		break;

	case 2:
		stbuf->st_mode = S_IFREG | 0444;
		stbuf->st_nlink = 1;
		stbuf->st_size = strlen(hello_str);
		break;

	default:
		return -1;
	}
	return 0;
}

static void hello_ll_lookup(fuse_req_t req, fuse_ino_t parent, const char *name)
{
	struct fuse_entry_param e;

	if (parent != 1 || strcmp(name, hello_name) != 0)
		fuse_reply_err(req, ENOENT);
	else {
		memset(&e, 0, sizeof(e));
		e.ino = 2;
		e.attr_timeout = 1.0;
		e.entry_timeout = 1.0;
		hello_stat(e.ino, &e.attr);

		fuse_reply_entry(req, &e);
	}
}

static void hello_ll_getattr(fuse_req_t req, fuse_ino_t ino,
			     struct fuse_file_info *fi)
{
	struct stat stbuf;

	(void) fi;

	memset(&stbuf, 0, sizeof(stbuf));
	if (hello_stat(ino, &stbuf) == -1)
		fuse_reply_err(req, ENOENT);
	else
		fuse_reply_attr(req, &stbuf, 1.0);
}


struct dirbuf {
	char *p;
	size_t size;
};

static void dirbuf_add(fuse_req_t req, struct dirbuf *b, const char *name,
		       fuse_ino_t ino)
{
	struct stat stbuf;
	size_t oldsize = b->size;
	b->size += fuse_add_direntry(req, NULL, 0, name, NULL, 0);
        char *newp = (char*) realloc(b->p, b->size);
        if (!newp) {
            fprintf(stderr, "*** fatal error: cannot allocate memory\n");
            abort();
        }
	b->p = newp;
	memset(&stbuf, 0, sizeof(stbuf));
	stbuf.st_ino = ino;
	fuse_add_direntry(req, b->p + oldsize, b->size - oldsize, name, &stbuf,
			  b->size);
}


static int reply_buf_limited(fuse_req_t req, const char *buf, size_t bufsize,
			     off_t off, size_t maxsize)
{
	if (off < bufsize)
		return fuse_reply_buf(req, buf + off,
				      std::min(bufsize - off, maxsize));
	else
		return fuse_reply_buf(req, NULL, 0);
}

static void hello_ll_readdir(fuse_req_t req, fuse_ino_t ino, size_t size,
			     off_t off, struct fuse_file_info *fi)
{
	(void) fi;

	if (ino != 1)
		fuse_reply_err(req, ENOTDIR);
	else {
		struct dirbuf b;

		memset(&b, 0, sizeof(b));
		dirbuf_add(req, &b, ".", 1);
		dirbuf_add(req, &b, "..", 1);
		dirbuf_add(req, &b, hello_name, 2);
		reply_buf_limited(req, b.p, b.size, off, size);
		free(b.p);
	}
}

static void hello_ll_open(fuse_req_t req, fuse_ino_t ino,
			  struct fuse_file_info *fi)
{
	if (ino != 2)
		fuse_reply_err(req, EISDIR);
	else if ((fi->flags & 3) != O_RDONLY)
		fuse_reply_err(req, EACCES);
	else
		fuse_reply_open(req, fi);
}

static void hello_ll_read(fuse_req_t req, fuse_ino_t ino, size_t size,
			  off_t off, struct fuse_file_info *fi)
{
	(void) fi;

	assert(ino == 2);
	reply_buf_limited(req, hello_str, strlen(hello_str), off, size);
}

static struct fuse_lowlevel_ops hello_ll_oper = {
	.lookup		= hello_ll_lookup,
	.getattr	= hello_ll_getattr,
	.open		= hello_ll_open,
	.read		= hello_ll_read,
	.readdir	= hello_ll_readdir,
};

struct Fuse::Impl {
    executor_type _ex;
    const fs::path _mountdir;
    std::thread _thread;
    net::executor_work_guard<executor_type> _work;
    std::mutex _mutex;
    ScopedCancel _scoped_cancel;

    fuse_session *_fuse_session = nullptr;
    fuse_chan *_fuse_channel = nullptr;
    fuse_args _fuse_args;

    Impl(executor_type ex, fs::path mountdir) :
        _ex(ex),
        _mountdir(std::move(mountdir)),
        _work(net::make_work_guard(_ex))
    {
        static const char* argv[] = { "ouisync" };

        _fuse_args = FUSE_ARGS_INIT(1, (char**) argv);

        _fuse_channel = fuse_mount(_mountdir.c_str(), &_fuse_args);
        if (!_fuse_channel) throw std::runtime_error("FUSE: Failed to mount");

        _fuse_session = fuse_lowlevel_new(nullptr, &hello_ll_oper, sizeof(hello_ll_oper), NULL);
        if (!_fuse_session) throw std::runtime_error("FUSE: Failed to initialize");

        fuse_session_add_chan(_fuse_session, _fuse_channel);

        _thread = std::thread([this] { run(); });
    }

    void run()
    {
        int err = fuse_session_loop(_fuse_session);
        if (err) throw std::runtime_error("FUSE: Session loop returned error");
    }

    void exit_loop()
    {
        fuse_session_exit(_fuse_session);
        auto c = _fuse_channel;
        _fuse_channel = nullptr;
        fuse_unmount(_mountdir.c_str(), c);
    }

    ~Impl()
    {
        _thread.join();
        fuse_opt_free_args(&_fuse_args);
        if (_fuse_channel) {
            fuse_session_remove_chan(_fuse_channel);
            fuse_unmount(_mountdir.c_str(), _fuse_channel);
        }
        if (_fuse_session) fuse_session_destroy(_fuse_session);
    }
};

Fuse::Fuse(executor_type ex, fs::path mountdir) :
    _impl(new Impl(ex, std::move(mountdir)))
{}

void Fuse::exit_loop()
{
    assert(_impl);
    if (!_impl) return;
    _impl->exit_loop();
    _impl = nullptr;
}

Fuse::~Fuse() {
    if (!_impl) return;
    exit_loop();
}

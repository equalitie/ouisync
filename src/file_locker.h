#pragma once

#include "file.h"
#include "barrier.h"

#include <boost/filesystem/path.hpp>
#include <boost/asio/execution/executor.hpp>

namespace ouisync {

class FileLocker {
public:
    using executor_type = net::any_io_executor;

private:
    enum class Type { Read, ReadAndWrite };

    struct FileBarrier : Barrier {
        Type type;

        FileBarrier(const FileBarrier&) = delete;
        FileBarrier(FileBarrier&&) = default;

        FileBarrier(Type type, const executor_type& ex) :
            Barrier(ex), type(type) {}
    };

    using BarrierMap = std::map<fs::path, std::shared_ptr<FileBarrier>>;

    struct FileBase {
        File file;
        Barrier::Lock lock;
        FileLocker* file_locker;
        BarrierMap::iterator barier_i;
        intrusive::list_hook hook;

        FileBase(
                File file,
                Barrier::Lock lock,
                FileLocker* file_locker,
                BarrierMap::iterator barier_i) :
            file(std::move(file)),
            lock(std::move(lock)),
            file_locker(file_locker),
            barier_i(std::move(barier_i))
        {}

        FileBase(FileBase&& other) :
            file(std::move(other.file)),
            lock(std::move(other.lock)),
            file_locker(other.file_locker),
            barier_i(std::move(other.barier_i))
        {
            other.file_locker = nullptr;
            hook.swap_nodes(other.hook);
        }

        ~FileBase() {
            if (!file_locker) return;
            auto barrier = barier_i->second;
            assert(barrier->lock_count());
            if (barrier->lock_count() <= 1) {
                file_locker->_barriers.erase(barier_i);
            }
        }
    };

public:
    class FileR : private FileBase {
        public:
        net::awaitable<size_t> read(net::mutable_buffer, Cancel);

        private:
        friend class FileLocker;
        FileR(FileBase base) : FileBase(std::move(base)) {}
    };

    class FileRW : private FileBase {
        public:
        net::awaitable<size_t> read(net::mutable_buffer, Cancel);
        net::awaitable<void>   write(net::const_buffer, Cancel);

        private:
        friend class FileLocker;
        FileRW(FileBase base) : FileBase(std::move(base)) {}
    };

public:
    FileLocker(net::io_context&);
    FileLocker(const executor_type&);

    FileLocker(const FileLocker&)       = delete;
    FileLocker(FileLocker&&)            = delete; // TODO?
    FileLocker& operator=(FileLocker&&) = delete; // TODO?

    net::awaitable<FileRW> truncate_or_create(const fs::path&, Cancel);
    net::awaitable<FileR>  open_readonly(const fs::path&, Cancel);

    ~FileLocker();

private:
    executor_type _ex;
    BarrierMap _barriers;
    intrusive::list<FileBase, &FileBase::hook> _files;
    ScopedCancel _scoped_cancel;
};

} // namespace

#include "file_locker.h"

#include <boost/filesystem/operations.hpp>

using namespace ouisync;

FileLocker::FileLocker(net::io_context& ioc) :
    _ex(ioc.get_executor())
{}

FileLocker::FileLocker(const executor_type& ex) :
    _ex(ex)
{}

net::awaitable<FileLocker::FileRW>
FileLocker::truncate_or_create(const fs::path& path_, Cancel cancel)
{
    auto sc = _scoped_cancel.connect(cancel);

    fs::path path = fs::canonical(path_.parent_path()) / path_.filename();

    while (true) {
        auto [iter, is_new] = _barriers.insert(std::make_pair(path, nullptr));
        auto& barrier = iter->second;

        if (!is_new) {
            auto b = barrier; // prevent from destruction while waiting;
            co_await b->wait(cancel);
            continue;
        }

        barrier = std::make_shared<FileBarrier>(Type::ReadAndWrite, _ex);

        FileRW f{{ File::truncate_or_create(_ex, path), barrier->lock(), this, iter }};
        _files.push_back(f);
        co_return f;
    }
}

net::awaitable<FileLocker::FileR>
FileLocker::open_readonly(const fs::path& path_, Cancel cancel)
{
    auto sc = _scoped_cancel.connect(cancel);

    fs::path path = fs::canonical(path_.parent_path()) / path_.filename();

    while (true) {
        auto [iter, is_new] = _barriers.insert(std::make_pair(path, nullptr));
        auto& barrier = iter->second;

        if (!is_new && barrier->type == Type::ReadAndWrite) {
            auto b = barrier; // prevent from destruction while waiting;
            co_await b->wait(cancel);
            continue;
        }

        if (is_new) {
            barrier = std::make_shared<FileBarrier>(Type::Read, _ex);
        }

        FileR f{ {File::open_readonly(_ex, path), barrier->lock(), this, iter } };
        _files.push_back(f);
        co_return f;
    }
}

FileLocker::~FileLocker()
{
    assert(_files.empty());
}

net::awaitable<size_t> FileLocker::FileR::read(net::mutable_buffer b, Cancel c)
{
    return file.read(b, c);
}

net::awaitable<size_t> FileLocker::FileRW::read(net::mutable_buffer b, Cancel c)
{
    return file.read(b, c);
}

net::awaitable<void> FileLocker::FileRW::write(net::const_buffer b, Cancel c)
{
    return file.write(b, c);
}

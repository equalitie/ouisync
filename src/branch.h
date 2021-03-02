#pragma once

#include "file_blob.h"
#include "directory.h"
#include "user_id.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"
#include "versioned_object.h"
#include "options.h"
#include "object_store.h"
#include "block_store.h"
#include "index.h"
#include "wait.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace ouisync {

class MultiDir;

class Branch {
private:
    class Op;
    class DirectoryOp;
    class HasTreeParrentOp;
    class FileOp;
    class RootOp;
    class BranchOp;
    class RemoveOp;
    class CdOp;

public:
    using executor_type = net::any_io_executor;

    class StateChangeWait {
      public:
        using Counter = uint64_t;

      public:
        StateChangeWait(executor_type ex)
            : _state_change_counter(0), _on_change(ex) {}

        [[nodiscard]] net::awaitable<Counter> wait(Opt<Counter> prev, Cancel cancel) {
            if (!prev || *prev >= _state_change_counter) {
                co_await _on_change.wait(cancel);
            }
            co_return _state_change_counter;
        }

        void notify() {
            _state_change_counter++;
            _on_change.notify();
        }

      private:
        Counter _state_change_counter;
        Wait _on_change;
    };

public:
    static
    Branch create(executor_type, const fs::path& path, UserId user_id, ObjectStore&, BlockStore&, Options::Branch);

    static
    Branch load(executor_type, const fs::path& file_path, UserId user_id, ObjectStore&, BlockStore&, Options::Branch);

    void mknod(PathRange);

    size_t write(PathRange, const char* buf, size_t size, size_t offset);

    bool remove(const fs::path&);
    bool remove(PathRange);

    size_t truncate(PathRange, size_t);

    void mkdir(PathRange);

    const UserId& user_id() const { return _user_id; }

    friend std::ostream& operator<<(std::ostream&, const Branch&);

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _index;
    }

    void sanity_check() {} // TODO

    const Index& index() const { return _index; }

    ObjectStore& objstore() const { return _objstore; }
    BlockStore& block_store() const { return _block_store; }

    void store(const BlockStore::Block&);

    StateChangeWait& on_change() { return _state_change_wait; }

    void merge_index(const Index&);

    const std::set<ObjectId>& missing_objects() const { return _index.missing_objects(); }

    std::set<std::string> readdir(PathRange path) const;

    FileSystemAttrib get_attr(PathRange path) const;

    size_t read(PathRange path, const char* buf, size_t size, size_t offset) const;

private:
    Branch(executor_type, const fs::path& file_path, const UserId&, ObjectStore&, BlockStore&, Options::Branch);

    void store_self() const;

    template<class F> void update_dir(PathRange, F&&);

    std::unique_ptr<DirectoryOp> root_op();
    std::unique_ptr<DirectoryOp> cd_into(PathRange);
    std::unique_ptr<FileOp> get_file(PathRange);

    MultiDir root_multi_dir() const;

    template<class OpPtr>
    void do_commit(OpPtr&);

    std::set<ObjectId> roots() const;

    void update_user_index(const UserId&, const Index&);

private:
    executor_type _ex;
    fs::path _file_path;
    Options::Branch _options;
    ObjectStore& _objstore;
    BlockStore& _block_store;
    UserId _user_id;
    Index _index;

    StateChangeWait _state_change_wait;
};

} // namespace

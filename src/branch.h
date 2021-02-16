#pragma once

#include "object_id.h"
#include "file_blob.h"
#include "directory.h"
#include "user_id.h"
#include "version_vector.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"
#include "commit.h"
#include "branch_view.h"
#include "options.h"
#include "object_store.h"
#include "index.h"
#include "wait.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace ouisync {

class Branch {
private:
    class Op;
    class TreeOp;
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
            if (!prev || *prev <= _state_change_counter) {
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
    using Indices = std::map<UserId, Index>;

public:
    static
    Branch create(executor_type, const fs::path& path, UserId user_id, ObjectStore&, Options::Branch);

    static
    Branch load(executor_type, const fs::path& file_path, UserId user_id, ObjectStore&, Options::Branch);

    const Commit& commit() const { return _indices.find(_user_id)->second.commit(); }

    const ObjectId& root_id() const { return commit().root_id; }

    BranchView branch_view() const;

    // XXX: Deprecated, use `write` instead. I believe these are currently
    // only being used in tests.
    void store(PathRange, const FileBlob&);
    void store(const fs::path&, const FileBlob&);

    size_t write(PathRange, const char* buf, size_t size, size_t offset);

    bool remove(const fs::path&);
    bool remove(PathRange);

    size_t truncate(PathRange, size_t);

    void mkdir(PathRange);

    const VersionVector& stamp() const { return commit().stamp; }

    const UserId& user_id() const { return _user_id; }

    friend std::ostream& operator<<(std::ostream&, const Branch&);

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _indices & _missing_objects;
    }

    void sanity_check() {} // TODO

    const Indices& indices() const { return _indices; }

    ObjectStore& objstore() const { return _objstore; }

    StateChangeWait& on_change() { return _state_change_wait; }

    void merge_indices(const Indices& indices);

    const std::set<ObjectId>& missing_objects() const { return _missing_objects; }

private:
    friend class BranchView;

    Branch(executor_type, const fs::path& file_path, const UserId&, ObjectStore&, Options::Branch);
    Branch(executor_type, const fs::path& file_path, const UserId&, Commit, ObjectStore&, Options::Branch);

    void store_self() const;

    template<class F> void update_dir(PathRange, F&&);

    std::unique_ptr<TreeOp> root();
    std::unique_ptr<TreeOp> cd_into(PathRange);
    std::unique_ptr<FileOp> get_file(PathRange);

    template<class OpPtr>
    void do_commit(OpPtr&);

    std::set<ObjectId> roots() const;

    void update_user_index(const UserId&, const Index&);

private:
    executor_type _ex;
    fs::path _file_path;
    Options::Branch _options;
    ObjectStore& _objstore;
    UserId _user_id;
    Indices _indices;
    std::set<ObjectId> _missing_objects;

    StateChangeWait _state_change_wait;
};

} // namespace

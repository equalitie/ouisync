#include "remote_branch.h"
#include "branch_io.h"
#include "branch_type.h"
#include "object/io.h"
#include "refcount.h"
#include "archive.h"
#include "snapshot.h"
#include "ouisync_assert.h"

#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

#include <iostream>

using namespace ouisync;

using std::move;
using std::make_pair;
using object::Tree;
using std::set;

/* static */
RemoteBranch RemoteBranch::load(fs::path filepath, Options::RemoteBranch options)
{
    RemoteBranch branch(filepath, move(options));
    archive::load(filepath, branch);
    return branch;
}

RemoteBranch::RemoteBranch(Commit commit, fs::path filepath, Options::RemoteBranch options) :
    _filepath(std::move(filepath)),
    _options(move(options)),
    _commit(std::move(commit))
{
    _missing_objects.insert({_commit.root_id, {}});
}

RemoteBranch::RemoteBranch(fs::path filepath, Options::RemoteBranch options) :
    _filepath(std::move(filepath)),
    _options(std::move(options))
{}

static std::set<ObjectId> _children(const Tree& tree)
{
    std::set<ObjectId> ret;
    for (auto& ch : tree) ret.insert(ch.second);
    return ret;
}

net::awaitable<ObjectId> RemoteBranch::insert_blob(const Blob& blob)
{
    auto id = object::io::store(_options.objectdir, blob);
    insert_object(id, {});
    store_self();
    co_return id;
}

net::awaitable<ObjectId> RemoteBranch::insert_tree(const Tree& tree)
{
    auto id = object::io::store(_options.objectdir, tree);
    insert_object(id, _children(tree));
    store_self();
    co_return id;
}

void RemoteBranch::filter_missing(std::set<ObjectId>& objs) const
{
    for (auto i = objs.begin(); i != objs.end();) {
        auto j = std::next(i);
        if (object::io::exists(_options.objectdir, *i)) objs.erase(i);
        i = j;
    }
}

void RemoteBranch::insert_object(const ObjectId& id, std::set<ObjectId> children)
{
    // Missing objects:    Object -> Parents
    // Incomplete objects: Object -> Children

    filter_missing(children);

    bool is_complete = children.empty();

    if (is_complete) {
        Rc::load(_options.objectdir, id).increment_recursive_count();
    } else {
        Rc::load(_options.objectdir, id).increment_direct_count();
    }

    auto parents = std::move(_missing_objects.at(id));
    _missing_objects.erase(id);

    if (children.empty()) {
        _complete_objects.insert(id);

        // Check that any of the parents of `id` became "complete".
        for (auto& parent : parents) {
            auto& missing_children = _incomplete_objects.at(parent);

            missing_children.erase(id);

            if (missing_children.empty()) {
                Rc rc = Rc::load(_options.objectdir, parent);
                rc.decrement_direct_count();
                rc.increment_recursive_count();

                _incomplete_objects.erase(parent);
                _complete_objects.insert(parent);

                // We no longer need to keep track of it as it's covered by the
                // parent.
                _complete_objects.erase(id);
            }
        }
    } else {
        for (auto& child : children) {
            _missing_objects[child].insert(id);
        }

        if (children.empty()) {
            _complete_objects.insert(id);
        } else {
            _incomplete_objects.insert({id, move(children)});
        }
    }
}

net::awaitable<void> RemoteBranch::introduce_commit(const Commit& commit)
{
    _commit = commit;

    // Missing objects don't increase refcount
    _missing_objects.clear();

    auto incomplete_objects = std::move(_incomplete_objects);
    auto complete_objects   = std::move(_complete_objects);


    for (auto& [id, _] : incomplete_objects) {
        refcount::flat_remove(_options.objectdir, id);
    }

    for (auto& id      : complete_objects) {
        refcount::deep_remove(_options.objectdir, id);
    }

    _missing_objects.insert({_commit.root_id, {}});

    store_self();
    co_return;
}

//--------------------------------------------------------------------
void RemoteBranch::sanity_check() const {
    for (auto& [id, _] : _incomplete_objects) {
        ouisync_assert(object::io::exists(_options.objectdir, id));
    }

    for (auto& id : _complete_objects) {
        ouisync_assert(object::io::is_complete(_options.objectdir, id));
    }
}

//--------------------------------------------------------------------

Snapshot RemoteBranch::create_snapshot() const
{
    auto snapshot = Snapshot::create(_commit, _options);

    if (_incomplete_objects.empty()) {
        for (auto& [id, _] : _incomplete_objects) {
            snapshot.capture_flat_object(id);
        }
    } else {
        for (auto& id : _complete_objects) {
            snapshot.capture_full_object(id);
        }
    }

    return snapshot;
}

//--------------------------------------------------------------------

void RemoteBranch::store_self() const {
    archive::store(_filepath, *this);
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const RemoteBranch& b)
{
    os << "RemoteBranch:\n";
    os << BranchIo::Immutable(b._options.objectdir, b._commit.root_id);
    os << "Complete objs: ";
    for (auto& id : b._complete_objects) {
        os << id << ", ";
    }
    os << "\nIncomplete objs: ";
    for (auto& [id, _] : b._incomplete_objects) {
        os << id << ", ";
    }
    return os << "\n";
}

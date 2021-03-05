#pragma once

#include "operation_interface.h"
#include "blob.h"

namespace ouisync {

class Branch::RootOp : public Branch::DirectoryOp {
public:
    RootOp(BlockStore& block_store, const UserId& this_user_id, Index& index) :
        _block_store(block_store),
        _this_user_id(this_user_id),
        _index(index),
        _original_commit(*_index.commit(this_user_id)),
        _multi_dir(_index.commits(), block_store)
    {
        auto blob = Blob::open(_original_commit.id, _block_store);
        if (!_tree.maybe_load(blob)) {
            throw std::runtime_error("Failed to parse block as a directory");
        }
    }

    Directory& tree() override {
        return _tree;
    }

    bool commit() override {
        auto new_id = _tree.calculate_id();
        auto old_id = _original_commit.id;

        if (old_id == new_id) return false;

        auto new_id_ = _tree.save(_block_store, _index);
        assert(new_id == new_id_);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _index.insert_object(_this_user_id, child_id, new_id);
            });

        _index.insert_object(_this_user_id, new_id, new_id);

        _index.set_version_vector(_this_user_id, _tree.calculate_version_vector_union());

        remove_recursive(old_id, old_id);

        return true;
    }

    Index& index() { return _index; }
    BlockStore& block_store() { return _block_store; }

    RootOp* root() override { return this; }

    void increment(VersionVector& vv) const {
        vv.set_version(_this_user_id, _original_commit.versions.version_of(_this_user_id) + 1);
    }

    void remove_recursive(const ObjectId& obj_id, const ObjectId& parent_id) {
        _index.remove_object(_this_user_id, obj_id, parent_id);

        if (_index.someone_has(obj_id)) return;

        auto blob = Blob::open(obj_id, _block_store);

        Directory d;
        if (d.maybe_load(blob)) {
            d.for_each_unique_child([&] (auto& filename, auto& child_id) {
                remove_recursive(child_id, obj_id);
            });
        }

        _block_store.remove(obj_id);
    }

    const MultiDir& multi_dir() const override { return _multi_dir; }

private:
    BlockStore& _block_store;
    UserId _this_user_id;
    Directory _tree;
    Index& _index;
    VersionedObject _original_commit;
    MultiDir _multi_dir;
};

} // namespace

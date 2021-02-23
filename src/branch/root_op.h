#pragma once

#include "operation_interface.h"

namespace ouisync {

class Branch::RootOp : public Branch::DirectoryOp {
public:
    RootOp(ObjectStore& objstore, const UserId& this_user_id, Index& index) :
        _objstore(objstore),
        _this_user_id(this_user_id),
        _index(index),
        _original_commit(*_index.commit(this_user_id)),
        _multi_dir(_index.commits(), objstore)
    {
        _tree = _objstore.load<Directory>(_original_commit.id);
    }

    Directory& tree() override {
        return _tree;
    }

    bool commit() override {
        auto new_id = _tree.calculate_id();
        auto old_id = _original_commit.id;

        if (old_id == new_id) return false;

        _objstore.store(_tree);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _index.insert_object(_this_user_id, child_id, new_id);
            });

        _index.insert_object(_this_user_id, new_id, new_id);

        _index.set_version_vector(_this_user_id, _tree.calculate_version_vector_union());

        remove_recursive(old_id, old_id);

        return true;
    }

    Index& index() { return _index; }
    ObjectStore& objstore() { return _objstore; }

    RootOp* root() override { return this; }

    void increment(VersionVector& vv) const {
        vv.set_version(_this_user_id, _original_commit.versions.version_of(_this_user_id) + 1);
    }

    void remove_recursive(const ObjectId& obj_id, const ObjectId& parent_id) {
        _index.remove_object(_this_user_id, obj_id, parent_id);

        if (_index.someone_has(obj_id)) return;

        auto obj = _objstore.load<Directory, FileBlob::Nothing>(obj_id);

        apply(obj,
                [&](const Directory& d) {
                    d.for_each_unique_child([&] (auto& filename, auto& child_id) {
                        remove_recursive(child_id, obj_id);
                    });
                },
                [&](const FileBlob::Nothing&) {
                });

        _objstore.remove(obj_id);
    }

    const MultiDir& multi_dir() const override { return _multi_dir; }

private:
    ObjectStore& _objstore;
    UserId _this_user_id;
    Directory _tree;
    Index& _index;
    VersionedObject _original_commit;
    MultiDir _multi_dir;
};

} // namespace

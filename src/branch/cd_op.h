#pragma once

#include "operation_interface.h"

namespace ouisync {

class Branch::CdOp : public Branch::DirectoryOp {
public:
    CdOp(std::unique_ptr<Branch::DirectoryOp> parent,
            const UserId& this_user_id, std::string dirname, bool create = false) :
        _parent(move(parent)),
        _root(_parent->root()),
        _this_user_id(this_user_id),
        _dirname(move(dirname)),
        _multi_dir(_parent->multi_dir().cd_into(_dirname))
    {
        auto opt_user_version = _multi_dir.pick_subdirectory_to_edit(this_user_id, _dirname);

        if (!opt_user_version) {
            if (!create) throw_error(sys::errc::no_such_file_or_directory);
            return; // One will be created in `commit()`
        }

        _old = opt_user_version->vobj;

        auto blob = Blob::open(_old->id, root()->block_store());

        if (!_tree.maybe_load(blob)) {
            throw std::runtime_error("Block doesn't represent a Directory");
        }
    }

    Directory& tree() override {
        return _tree;
    }

    bool commit() override {
        auto new_tree_id = _tree.save(_root->transaction());

        if (_old && _old->id == new_tree_id) {
            return false;
        }

        VersionVector new_vv;

        if (_old) {
            // XXX: We should make a union of all `_dirname`s per each users
            // that we've have downloaded *completely*.
            new_vv = _old->versions;
        }

        root()->increment(new_vv);

        _parent->tree()[_dirname][_this_user_id] = { new_tree_id, std::move(new_vv) };

        return _parent->commit();
    }

    RootOp* root() override { return _root; }

    const MultiDir& multi_dir() const override { return _multi_dir; }

private:
    std::unique_ptr<DirectoryOp> _parent;
    RootOp* _root;
    UserId _this_user_id;
    std::string _dirname;
    Opt<VersionedObject> _old;
    Directory _tree;
    MultiDir _multi_dir;
};

} // namespace

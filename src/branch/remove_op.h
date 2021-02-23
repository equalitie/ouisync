#pragma once

#include "operation_interface.h"

namespace ouisync {

// XXX: This is just a stub, proper implementation needs to create an entry
// that marks the file/directory as removed. Otherwise concurrent edits would
// always re-add that directory back.
class Branch::RemoveOp : public Branch::Op {
public:
    RemoveOp(std::unique_ptr<Branch::DirectoryOp> parent, std::string filename) :
        _parent(move(parent))
    {
        _tree_entry = _parent->tree().find(filename);

        if (!_tree_entry) {
            throw_error(sys::errc::no_such_file_or_directory);
        }
    }

    bool commit() override {
        _parent->tree().erase(_tree_entry);
        return _parent->commit();
    }

    RootOp* root() override { return _parent->root(); }

private:
    std::unique_ptr<Branch::DirectoryOp> _parent;
    Directory::MutableHandle _tree_entry;
};

} // namespace

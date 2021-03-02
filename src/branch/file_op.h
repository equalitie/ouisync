#pragma once

#include "operation_interface.h"

namespace ouisync {

class Branch::FileOp : public Branch::Op {
public:
    FileOp(std::unique_ptr<DirectoryOp> parent, const UserId& this_user_id, std::string filename) :
        _parent(move(parent)),
        _this_user_id(this_user_id),
        _filename(move(filename))
    {
        auto per_name = _parent->tree().find(_filename);

        if (!per_name) {
            return;
        }

        auto i = per_name.find(_this_user_id);

        if (i != per_name.end()) {
            _old = i->second;
            _blob = FileBlob{};
            _blob->load(root()->block_store().load(_old->id));
        }
    }

    Opt<FileBlob>& blob() { return _blob; }

    bool commit() override {
        if (!_blob && !_old) return false;

        if (!_blob) { // Was removed
            // We need a mark that the file was removed.
            assert("TODO" && 0);
        }

        auto new_id = _blob->calculate_id();

        if (_old && _old->id == new_id) return false;

        _blob->save(root()->block_store());

        VersionVector vv;

        if (_old) {
            vv = _old->versions;
        }

        root()->increment(vv);

        _parent->tree()[_filename][_this_user_id] = { new_id, std::move(vv) };

        return _parent->commit();
    }

    RootOp* root() override { return _parent->root(); }

private:
    std::unique_ptr<DirectoryOp> _parent;
    UserId _this_user_id;
    std::string _filename;
    Opt<VersionedObject> _old;
    Opt<FileBlob> _blob;
};

} // namespace

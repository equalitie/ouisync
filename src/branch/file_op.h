#pragma once

#include "operation_interface.h"
#include "transaction.h"

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
            auto blob = Blob::open(_old->id, root()->block_store());
            _file = File::open(blob);
        }
    }

    Opt<File>& file() { return _file; }

    bool commit() override {
        if (!_file && !_old) return false;

        if (!_file) { // Was removed
            // We need a mark that the file was removed.
            assert("TODO" && 0);
        }

        auto new_id = _file->calculate_id();

        if (_old && _old->id == new_id) return false;

        _file->save(root()->transaction());

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
    Opt<File> _file;
};

} // namespace

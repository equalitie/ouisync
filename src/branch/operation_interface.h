#pragma once

#include "../branch.h"

namespace ouisync {

class Branch::Op {
    public:
    virtual bool commit() = 0;
    virtual Branch::RootOp* root() = 0;
    virtual ~Op() {}
};

class Branch::DirectoryOp : public Branch::Op {
    public:
    virtual Directory& tree() = 0;
    virtual const MultiDir& multi_dir() const = 0;
    virtual ~DirectoryOp() {};
};

} // namespace

#pragma once

#include "branch.h"
#include "block_id.h"
#include "shortcuts.h"

namespace ouisync {
    class Branch;
}

namespace ouisync {

class BlockStore {
public:
    using Data = std::vector<uint8_t>;

public:
    BlockStore(const fs::path& basedir);

    bool tryCreate(const BlockId &blockId, const Data &data);

    bool remove(const BlockId &blockId);

    boost::optional<Data> load(const BlockId &blockId) const;

    // Store the block with the given blockId. If it doesn't exist, it is created.
    void store(const BlockId &blockId, const Data &data);

    virtual ~BlockStore();

private:
    fs::path _branchdir;
    fs::path _objdir;
    std::unique_ptr<Branch> _branch;
    UserId _user_id;
    std::mutex _mutex;
};

} // namespace

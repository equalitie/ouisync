#pragma once

#include "object/id.h"

#include <blockstore/interface/BlockStore2.h>
#include <namespaces.h>
#include <boost/uuid/uuid.hpp>

namespace blockstore {
    namespace ondisk {
        class OnDiskBlockStore2;
    }
}

namespace ouisync {
    class Root;
}

namespace ouisync {

class BlockStore : public blockstore::BlockStore2 {
public:
    using BlockId = blockstore::BlockId;

public:
    BlockStore(const fs::path& basedir);

    WARN_UNUSED_RESULT
    bool tryCreate(const BlockId &blockId, const cpputils::Data &data) override;

    WARN_UNUSED_RESULT
    bool remove(const BlockId &blockId) override;

    WARN_UNUSED_RESULT
    boost::optional<cpputils::Data> load(const BlockId &blockId) const override;

    // Store the block with the given blockId. If it doesn't exist, it is created.
    void store(const BlockId &blockId, const cpputils::Data &data) override;

    uint64_t numBlocks() const override;

    uint64_t estimateNumFreeBytes() const override;

    uint64_t blockSizeFromPhysicalBlockSize(uint64_t blockSize) const override;

    void forEachBlock(std::function<void (const BlockId &)> callback) const override;

    virtual ~BlockStore();

private:
    fs::path _rootdir;
    fs::path _objdir;
    std::unique_ptr<Root> _root;
    Uuid _user_id;
    std::mutex _mutex;
};

} // namespace

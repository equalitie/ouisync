#pragma once

#include <blockstore/interface/BlockStore2.h>
#include <namespaces.h>

namespace blockstore {
    namespace ondisk {
        class OnDiskBlockStore2;
    }
}

namespace ouisync {
    class BlockSync;
}

namespace ouisync {

class BlockStore : public blockstore::BlockStore2 {
public:
    using BlockId = blockstore::BlockId;

public:
    BlockStore(const fs::path& basedir, std::unique_ptr<BlockSync> sync);

    WARN_UNUSED_RESULT
    bool tryCreate(const BlockId &blockId, const cpputils::Data &data) override;

    WARN_UNUSED_RESULT
    bool remove(const BlockId &blockId) override;

    WARN_UNUSED_RESULT
    boost::optional<cpputils::Data> load(const BlockId &blockId) const override;

    // Store the block with the given blockId. If it doesn't exist, it is created.
    void store(const BlockId &blockId, const cpputils::Data &data);

    uint64_t numBlocks() const override;

    uint64_t estimateNumFreeBytes() const override;

    uint64_t blockSizeFromPhysicalBlockSize(uint64_t blockSize) const override;

    void forEachBlock(std::function<void (const BlockId &)> callback) const override;

    virtual ~BlockStore();

private:

    fs::path _get_dir_for_block(const BlockId&) const;
    fs::path _get_data_file_path(const BlockId&) const;

private:
    fs::path _rootdir;
    std::unique_ptr<BlockSync> _sync;
};

} // namespace

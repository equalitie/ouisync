#pragma once

#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <namespaces.h>

namespace ouisync {

class BlockStore : public blockstore::BlockStore2 {
public:
    using BlockId = blockstore::BlockId;

public:
    BlockStore(fs::path basedir)
        : _bs(std::move(basedir))
    {}

    BlockId createBlockId() const override {
        auto r = _bs.createBlockId();
        std::cerr << "createBlockId() -> " << r.ToString() << "\n";
        return r;
    }

    WARN_UNUSED_RESULT
    bool tryCreate(const BlockId &blockId, const cpputils::Data &data) override {
        bool b = _bs.tryCreate(blockId, data);
        std::cerr << "tryCreate(" << blockId.ToString() << ") -> " << b << "\n";
        return b;
    }

    WARN_UNUSED_RESULT
    bool remove(const BlockId &blockId) override {
        auto b = _bs.remove(blockId);
        std::cerr << "remove(" << blockId.ToString() << ") -> " << b << "\n";
        return b;
    }

    WARN_UNUSED_RESULT
    boost::optional<cpputils::Data> load(const BlockId &blockId) const override {
        std::cerr << "load(" << blockId.ToString() << ")\n";
        return _bs.load(blockId);
    }

    // Store the block with the given blockId. If it doesn't exist, it is created.
    void store(const BlockId &blockId, const cpputils::Data &data) {
        std::cerr << "store(" << blockId.ToString() << ")\n";
        return _bs.store(blockId, data);
    }

    uint64_t numBlocks() const override {
        auto r = _bs.numBlocks();
        std::cerr << "numBlocks() -> " << r << "\n";
        return r;
    }

    uint64_t estimateNumFreeBytes() const override {
        auto r = _bs.estimateNumFreeBytes();
        std::cerr << "estimateNumFreeBytes() -> " << r << "\n";
        return r;
    }

    uint64_t blockSizeFromPhysicalBlockSize(uint64_t blockSize) const override {
        auto r = _bs.blockSizeFromPhysicalBlockSize(blockSize);
        std::cerr << "blockSizeFromPhysicalBlockSize(" << blockSize << ") -> " << r  << "\n";
        return r;
    }

    void forEachBlock(std::function<void (const BlockId &)> callback) const override {
        std::cerr << "forEachBlock\n";
        _bs.forEachBlock(std::move(callback));
    }

    virtual ~BlockStore() {}

private:
    blockstore::ondisk::OnDiskBlockStore2 _bs;
};

} // namespace

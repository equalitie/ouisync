#include <block_store.h>
#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>

using namespace ouisync;

using BlockId = blockstore::BlockId;

BlockStore::BlockStore(fs::path basedir)
    : _bs(std::make_unique<blockstore::ondisk::OnDiskBlockStore2>(std::move(basedir)))
{}

BlockId BlockStore::createBlockId() const {
    auto r = _bs->createBlockId();
    std::cerr << "createBlockId() -> " << r.ToString() << "\n";
    return r;
}

bool BlockStore::tryCreate(const BlockId &blockId, const cpputils::Data &data) {
    bool b = _bs->tryCreate(blockId, data);
    std::cerr << "tryCreate(" << blockId.ToString() << ") -> " << b << "\n";
    return b;
}

bool BlockStore::remove(const BlockId &blockId) {
    auto b = _bs->remove(blockId);
    std::cerr << "remove(" << blockId.ToString() << ") -> " << b << "\n";
    return b;
}

boost::optional<cpputils::Data> BlockStore::load(const BlockId &blockId) const {
    std::cerr << "load(" << blockId.ToString() << ")\n";
    return _bs->load(blockId);
}

// Store the block with the given blockId. If it doesn't exist, it is created.
void BlockStore::store(const BlockId &blockId, const cpputils::Data &data) {
    std::cerr << "store(" << blockId.ToString() << ")\n";
    return _bs->store(blockId, data);
}

uint64_t BlockStore::numBlocks() const {
    auto r = _bs->numBlocks();
    std::cerr << "numBlocks() -> " << r << "\n";
    return r;
}

uint64_t BlockStore::estimateNumFreeBytes() const {
    auto r = _bs->estimateNumFreeBytes();
    std::cerr << "estimateNumFreeBytes() -> " << r << "\n";
    return r;
}

uint64_t BlockStore::blockSizeFromPhysicalBlockSize(uint64_t blockSize) const {
    auto r = _bs->blockSizeFromPhysicalBlockSize(blockSize);
    std::cerr << "blockSizeFromPhysicalBlockSize(" << blockSize << ") -> " << r  << "\n";
    return r;
}

void BlockStore::forEachBlock(std::function<void (const BlockId &)> callback) const {
    std::cerr << "forEachBlock\n";
    _bs->forEachBlock(std::move(callback));
}

BlockStore::~BlockStore() {}

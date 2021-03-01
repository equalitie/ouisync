#include "block_store.h"
#include "object_store.h"

using namespace ouisync;
using std::move;

using Block = BlockStore::Block;

BlockStore::BlockStore(ObjectStore& objstore) :
    _objstore(objstore)
{}

Block BlockStore::load(const ObjectId& block_id)
{
    auto path = id_to_path(block_id);
    auto size = fs::file_size(path);
    Block block;
    block.resize(size);

    fs::ifstream ifs(path, fs::ifstream::binary);

    if (!ifs.is_open()) {
        throw std::runtime_error("archive::load: Failed to open object");
    }

    ifs.read(block.data(), block.size());

    return block;
}

Opt<Block> BlockStore::maybe_load(const ObjectId& block_id)
{
    auto path = id_to_path(block_id);

    if (!fs::exists(path)) return boost::none;

    auto size = fs::file_size(path);

    fs::ifstream ifs(path, fs::ifstream::binary);

    if (!ifs.is_open()) {
        throw std::runtime_error("archive::load: Failed to open object");
    }

    Block block;
    block.resize(size);
    ifs.read(block.data(), block.size());
    return block;
}

ObjectId BlockStore::store(const Block& block)
{
    auto id = calculate_block_id(block);

    fs::ofstream ofs(id_to_path(id), ofs.out | ofs.binary | ofs.trunc);
    ofs.write(block.data(), block.size());

    return id;
}

void BlockStore::store(const ObjectId& id, const Block& block)
{
    assert(id == calculate_block_id(block));
    fs::ofstream ofs(id_to_path(id), ofs.out | ofs.binary | ofs.trunc);
    ofs.write(block.data(), block.size());
}

fs::path BlockStore::id_to_path(const ObjectId& id)
{
    return _objstore.id_to_path(id);
}

/* static */
ObjectId BlockStore::calculate_block_id(const Block& block)
{
    Sha256 hash;
    hash.update(block.data(), block.size());
    return hash.close();
}

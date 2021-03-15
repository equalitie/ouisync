#include "block_store.h"
#include "hex.h"
#include "ouisync_assert.h"
#include <iostream>

#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp> // create_directories

using namespace ouisync;
using std::move;

BlockStore::BlockStore(const fs::path& blockdir) :
    _blockdir(blockdir)
{}

/* static */
Block BlockStore::load(const fs::path& path)
{
    auto size = fs::file_size(path);
    Block block;
    block.resize(size);

    fs::ifstream ifs(path, fs::ifstream::binary);

    if (!ifs.is_open()) {
        throw std::runtime_error("archive::load: Failed to open block");
    }

    ifs.read(block.data(), block.size());

    return block;
}

Block BlockStore::load(const BlockId& block_id) const
{
    return load(id_to_path(block_id));
}

Opt<Block> BlockStore::maybe_load(const BlockId& block_id) const
{
    auto path = id_to_path(block_id);
    if (!fs::exists(path)) return boost::none;
    return load(path);
}

void BlockStore::store(const BlockId& id, const char* data, size_t size)
{
    auto path = id_to_path(id);

    // XXX: if this probes every single directory in path, then it might be
    // slow and in such case we could instead try to create only the last 2.
    fs::create_directories(path.parent_path());

    fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);

    if (!ofs.is_open()) {
        throw std::runtime_error("archive::store: Failed to open file for writing");
    }

    ofs.write(data, size);
}

BlockId BlockStore::store(const char* data, size_t size)
{
    auto id = calculate_block_id(data, size);
    store(id, data, size);
    return id;
}

BlockId BlockStore::store(const Block& block)
{
    auto id = calculate_block_id(block);
    store(id, block.data(), block.size());
    return id;
}

void BlockStore::store(const BlockId& id, const Block& block)
{
    ouisync_assert(id == calculate_block_id(block));
    store(id, block.data(), block.size());
}

void BlockStore::remove(const BlockId& block_id)
{
    sys::error_code ec;
    fs::remove(id_to_path(block_id), ec);
    return;
}

fs::path BlockStore::id_to_path(const BlockId& id) const
{
    auto hex = to_hex<char>(id);
    string_view hex_sv{hex.data(), hex.size()};

    static const size_t prefix_size = 3;

    auto prefix = hex_sv.substr(0, prefix_size);
    auto rest   = hex_sv.substr(prefix_size, hex_sv.size() - prefix_size);

    return _blockdir / fs::path(prefix.begin(), prefix.end()) / fs::path(rest.begin(), rest.end());
}

/* static */
BlockId BlockStore::calculate_block_id(const Block& block)
{
    return calculate_block_id(block.data(), block.size());
}

/* static */
BlockId BlockStore::calculate_block_id(const char* data, size_t size)
{
    Sha256 hash;
    hash.update(data, size);
    return hash.close();
}

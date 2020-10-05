#include <block_store.h>
#include <block_sync.h>
#include "hash_store.h"
#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <boost/filesystem.hpp>
#include <cpp-utils/system/diskspace.h>

using namespace ouisync;
using std::move;
using std::unique_ptr;
using boost::optional;
using cpputils::Data;

namespace {
    constexpr size_t BLOCK_ID_HEX_SIZE = BlockId::STRING_LENGTH;

    /*
     * Unlike in original CryFS, we want the directory structure to also
     * represent a kind of Merkle tree (or even multiple of them: one per
     * branch) and thus to be able to quickly calculate hashes of each node we
     * want to limit the count of its children.
     *
     * Say the number of children per node is 2^8 (=256) and say we expect to
     * store 2^39 (~=500G) bytes of encrypted data. Each block is 2^15 (~=32K)
     * bytes in size. Thus we'll have 2^39/2^15 = 2^24 (~=16M) blocks.
     *
     * If the first level of the tree is represented by two hex characters,
     * this leaves 2^24/2^8 = 2^16 blocks for the second level.
     *
     * Then 2^16/2^8 = 2^8 for the third level, and that's the granularity we
     * want to achieve. So we can stop there.
     */
    constexpr size_t block_id_part_hex_size(unsigned part) {
        static_assert(BLOCK_ID_HEX_SIZE == 32, "BlockId size has changed");

        constexpr unsigned max_depth = 2;

        if (part <  max_depth) return 2;
        if (part == max_depth) return BLOCK_ID_HEX_SIZE - (max_depth*2);

        return 0;
    }
}

fs::path BlockStore::_get_dir_for_block(const BlockId &block_id) const {
    std::string block_id_str = block_id.ToString();
    fs::path path = _rootdir;
    unsigned part = 0;
    size_t start = 0;
    while (auto s = block_id_part_hex_size(part++)) {
        path /= block_id_str.substr(start, s);
        start += s;
    }
    return path;
}

fs::path BlockStore::_get_data_file_path(const BlockId &block_id) const {
    return _get_dir_for_block(block_id) / "data";
}

static inline auto create_digest(const cpputils::Data& data) {
    Sha256 hash;
    hash.update(data.data(), data.size());
    return hash.close();
}

BlockStore::BlockStore(const fs::path& basedir, unique_ptr<BlockSync> sync)
    : _rootdir(basedir / "blocks")
    , _sync(move(sync))
{
    fs::create_directories(_rootdir);
}

bool BlockStore::tryCreate(const BlockId &blockId, const Data &data) {
    auto filepath = _get_data_file_path(blockId);
    if (fs::exists(filepath)) {
        return false;
    }

    store(blockId, data);
    return true;
}

bool BlockStore::remove(const BlockId &blockId) {
    auto filepath = _get_data_file_path(blockId);
    if (!fs::is_regular_file(filepath)) { // TODO Is this branch necessary?
        return false;
    }
    bool retval = fs::remove(filepath);
    if (!retval) {
        cpputils::logging::LOG(cpputils::logging::ERR, "Couldn't find block {} to remove", blockId.ToString());
        return false;
    }
    if (fs::is_empty(filepath.parent_path())) {
        fs::remove(filepath.parent_path());
    }

    _sync->add_action(BlockSync::ActionRemoveBlock{blockId});

    return true;
}

optional<Data> BlockStore::load(const BlockId &blockId) const {
    auto fileContent = Data::LoadFromFile(_get_data_file_path(blockId));
    if (!fileContent) {
        return boost::none;
    }
    return {move(*fileContent)};
}

void BlockStore::store(const BlockId &block_id, const Data &data) {
  Data fileContent(data.size());
  std::memcpy(fileContent.data(), data.data(), data.size());
  auto filepath = _get_data_file_path(block_id);
  fs::create_directories(filepath.parent_path());
  fileContent.StoreToFile(filepath);

  auto digest = create_digest(data);

  HashStore::store(_get_dir_for_block(block_id) / "hashes", block_id, digest);

  _sync->add_action(BlockSync::ActionModifyBlock{block_id, digest});
}

uint64_t BlockStore::numBlocks() const {
    fs::recursive_directory_iterator i(_rootdir);

    uint64_t count = 0;

    for (auto i : fs::recursive_directory_iterator(_rootdir)) {
        if (fs::is_directory(i.path())) {
            continue;
        }
        if (i.path().filename() != "data") {
            continue;
        }
        ++count;
    }

    return count;
}

uint64_t BlockStore::estimateNumFreeBytes() const {
	return cpputils::free_disk_space_in_bytes(_rootdir);
}

uint64_t BlockStore::blockSizeFromPhysicalBlockSize(uint64_t blockSize) const {
    return blockSize;
}

static
BlockId reconstruct_id_from_block_path(const fs::path& root, fs::path p)
{
    p = p.parent_path(); // Remove the file name

    auto pi = p.begin();

    // Ignore the directories contained in root
    for (auto _ : root) { ++pi; }

    std::string str_id;

    while (pi != p.end()) {
        str_id += pi->string();
        ++pi;
    }

    return BlockId::FromString(str_id);
}

void BlockStore::forEachBlock(std::function<void (const BlockId &)> callback) const {
    assert(0 && "TODO: Test this");
    fs::recursive_directory_iterator i(_rootdir);

    for (auto i : fs::recursive_directory_iterator(_rootdir)) {
        if (fs::is_directory(i.path())) {
            continue;
        }
        if (i.path().filename() != "data") {
            continue;
        }
        callback(reconstruct_id_from_block_path(_rootdir, i.path()));
    }
}

BlockStore::~BlockStore() {}

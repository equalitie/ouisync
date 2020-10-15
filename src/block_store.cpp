#include "block_store.h"
#include "array_io.h"
#include "hex.h"
#include "version_vector.h"
#include "object/block.h"
#include "object/tree.h"
#include "object/io.h"

#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <boost/filesystem/operations.hpp>

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

static
fs::path _get_data_file_path(const BlockId &block_id) {
    std::string block_id_str = block_id.ToString();
    fs::path path;
    unsigned part = 0;
    size_t start = 0;
    while (auto s = block_id_part_hex_size(part++)) {
        if (start == 0) {
            path = block_id_str.substr(start, s);
        } else {
            path /= block_id_str.substr(start, s);
        }
        start += s;
    }
    return path;
}

BlockStore::BlockStore(const fs::path& basedir) :
    _branchdir(basedir / "branches"),
    _objdir(basedir / "objects")
{
    fs::create_directories(_branchdir);
    fs::create_directories(_objdir);

    _user_id = UserId::load_or_create(basedir / "user_id");
    _branch = std::make_unique<Branch>(Branch::load_or_create(_branchdir, _objdir, _user_id));
}

bool BlockStore::tryCreate(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto filepath = _get_data_file_path(block_id);

    object::Block block(data);
    auto id = object::io::maybe_store(_objdir, _branch->root_object_id(), filepath, block);

    if (id) {
        _branch->root_object_id(*id);
        return true;
    }

    return false;
}

bool BlockStore::remove(const BlockId &block_id) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto opt_new_id = object::io::remove(_objdir, _branch->root_object_id(), _get_data_file_path(block_id));
    if (!opt_new_id) return false;
    _branch->root_object_id(*opt_new_id);

    return true;
}

optional<Data> BlockStore::load(const BlockId &block_id) const {
    std::scoped_lock<std::mutex> lock(const_cast<std::mutex&>(_mutex));
    try {
        auto path = _get_data_file_path(block_id);
        auto block = object::io::load(_objdir, _branch->root_object_id(), path);
        return {move(*block.data())};
    } catch (const std::exception&) {
        // XXX: need to distinguis between "not found" and any other error.
        // I think the former should result in boost::none while the latter
        // should rethrow. But this needs to be checked as well.
        return boost::none;
    }
}

void list(const fs::path& objdir, object::Id id, std::string pad = "") {
    auto obj = object::io::load<object::Tree, object::Block>(objdir, id);

    apply(obj,
            [&] (const object::Tree& tree) {
                std::cerr << pad << tree << "\n";
                pad += "  ";
                for (auto p : tree) {
                    list(objdir, p.second, pad);
                }
            },
            [&] (const auto& o) {
                std::cerr << pad << o << "\n";
            });
}

void BlockStore::store(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto filepath = _get_data_file_path(block_id);

    object::Block block(data);
    auto id = object::io::store(_objdir, _branch->root_object_id(), filepath, block);
    _branch->root_object_id(id);
}

namespace {
    using HexBlockId = std::array<char, BlockId::STRING_LENGTH>;
}

template<class F>
static
void _for_each_block(const fs::path& objdir, object::Id id, const F& f, HexBlockId& hex_block_id, size_t start)
{
    auto obj = object::io::load<object::Tree, object::Block>(objdir, id);

    apply(obj,
            [&] (const object::Tree& tree) {
                for (auto& [name, obj_id] : tree) {
                    if (start + name.size() > hex_block_id.size()) { assert(0); continue; }
                    memcpy(hex_block_id.data() + start, name.data(), name.size());
                    _for_each_block(objdir, obj_id, f, hex_block_id, start + name.size());
                }
            },
            [&] (const auto& o) {
                if (start != hex_block_id.size()) { assert(0); return; }
                auto block_id = from_hex<char>(hex_block_id);
                if (!block_id) { assert(0); return; }
                f(BlockId::FromBinary(block_id->data()));
            });
}

template<class F>
static
void _for_each_block(const fs::path& objdir, object::Id id, const F& f)
{
    HexBlockId hex_block_id;
    return _for_each_block(objdir, id, f, hex_block_id, 0);
}

uint64_t BlockStore::numBlocks() const {
    uint64_t count = 0;
    _for_each_block(_objdir, _branch->root_object_id(), [&] (const auto&) { ++count; });
    return count;
}

uint64_t BlockStore::estimateNumFreeBytes() const {
	return cpputils::free_disk_space_in_bytes(_objdir);
}

uint64_t BlockStore::blockSizeFromPhysicalBlockSize(uint64_t blockSize) const {
    return blockSize;
}

void BlockStore::forEachBlock(std::function<void (const BlockId &)> callback) const {
    _for_each_block(_objdir, _branch->root_object_id(), callback);
}

BlockStore::~BlockStore() {}

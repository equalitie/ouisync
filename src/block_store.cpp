#include "block_store.h"
#include "array_io.h"
#include "hex.h"
#include "version_vector.h"
#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"

#include <boost/filesystem/operations.hpp>

using namespace ouisync;
using std::move;
using std::unique_ptr;
using boost::optional;

namespace {
    constexpr size_t BLOCK_ID_HEX_SIZE = 2 *
        std::tuple_size<BlockId>::value * sizeof(BlockId::value_type);

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
    auto hex_block_id = to_hex<char>(block_id);
    fs::path path;
    unsigned part = 0;
    size_t start = 0;
    while (auto size = block_id_part_hex_size(part++)) {
        string_view sv(hex_block_id.data() + start, size);

        if (start == 0) {
            path = sv.to_string();
        } else {
            path /= sv.to_string();
        }
        start += size;
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
    auto path = _get_data_file_path(block_id);
    return _branch->maybe_store(path, data);
}

bool BlockStore::remove(const BlockId &block_id) {
    std::scoped_lock<std::mutex> lock(_mutex);
    return _branch->remove(_get_data_file_path(block_id));
}

optional<BlockStore::Data> BlockStore::load(const BlockId &block_id) const {
    std::scoped_lock<std::mutex> lock(const_cast<std::mutex&>(_mutex));
    return _branch->maybe_load(_get_data_file_path(block_id));
}

void BlockStore::store(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);
    _branch->store(_get_data_file_path(block_id), data);
}

BlockStore::~BlockStore() {}

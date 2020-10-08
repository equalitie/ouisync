#include <block_store.h>
#include <block_sync.h>
#include "array_io.h"
#include "hex.h"
#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <boost/filesystem.hpp>

#include <boost/archive/text_oarchive.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <boost/uuid/uuid_serialize.hpp>

#include <cpp-utils/system/diskspace.h>
#include "object/any.h"

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
fs::path _get_dir_for_block(const BlockId &block_id) {
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

static
fs::path _get_data_file_path(const BlockId &block_id) {
    return _get_dir_for_block(block_id) / "data";
}

static inline auto create_digest(const cpputils::Data& data) {
    Sha256 hash;
    hash.update(data.data(), data.size());
    return hash.close();
}

BlockStore::BlockStore(const fs::path& basedir, unique_ptr<BlockSync> sync) :
    _rootdir(basedir / "blocks"),
    _objdir(basedir / "objects"),
    _sync(move(sync))
{
    fs::create_directories(_rootdir);
}

bool BlockStore::tryCreate(const BlockId &blockId, const Data &data) {
    auto filepath = _rootdir/_get_data_file_path(blockId);
    if (fs::exists(filepath)) {
        return false;
    }

    store(blockId, data);
    return true;
}

bool BlockStore::remove(const BlockId &blockId) {
    auto filepath = _rootdir/_get_data_file_path(blockId);
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
    auto fileContent = Data::LoadFromFile(_rootdir/_get_data_file_path(blockId));
    if (!fileContent) {
        return boost::none;
    }
    return {move(*fileContent)};
}

using Hashes = std::map<std::string, Sha256::Digest>;

static Sha256::Digest map_digest(const Hashes& hashes) {
    Sha256 hash;
    for (auto& [k, v] : hashes) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

static Hashes load_hashes(const fs::path& path)
{
    Hashes result;

    fs::ifstream ifs(path);
    if (!ifs.is_open()) return result;
    boost::archive::text_iarchive ia(ifs);
    ia >> result;
    return result;
}

static void store_hashes(const fs::path& path, const Hashes& map)
{
    fs::ofstream ofs(path);
    boost::archive::text_oarchive oa(ofs);
    oa << map;
}

void BlockStore::store(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto filepath = _rootdir/_get_data_file_path(block_id);
    fs::create_directories(filepath.parent_path());
    data.StoreToFile(filepath);

    auto digest = create_digest(data);
    auto dir = _get_dir_for_block(block_id);

    //{
    //    std::cerr << "1!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n";
    //    objects::Tree tree;
    //    auto d = tree.calculate_digest();
    //    auto r = tree.store(_objdir);
    //    std::cerr << "2!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!! " << _objdir << " "  << bool(r) << "\n";
    //    assert(r);
    //    auto o = objects::Any::load(_objdir, d);
    //    std::cerr << "3!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n";
    //    assert(o);
    //    std::cerr << "4!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!! " << bool(o->as_block()) << " " << bool(o->as_tree()) << "\n";
    //    assert(o->as_tree());
    //    std::cerr << "5!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n";
    //}
    //{
    //    do {
    //        auto f = dir.filename().string();
    //        dir.remove_filename();
    //        auto hash_file = _rootdir / dir / "hashes";
    //        auto hashes = load_hashes(hash_file);
    //        hashes[f] = digest;
    //        store_hashes(hash_file, hashes);
    //        digest = map_digest(hashes);
    //    }
    //    while (!dir.empty());
    //}

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

#include "block_store.h"
#include "block_sync.h"
#include "array_io.h"
#include "hex.h"
#include "defer.h"

#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <boost/filesystem.hpp>

#include <boost/range/iterator_range.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <boost/uuid/uuid_serialize.hpp>

#include <cpp-utils/system/diskspace.h>
#include "object/block.h"
#include "object/tree.h"
#include "object/load.h"
#include "object/remove.h"

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

static inline auto create_digest(const cpputils::Data& data) {
    Sha256 hash;
    hash.update(data.data(), data.size());
    return hash.close();
}

class ouisync::RootId {
public:
    static Opt<RootId> load(const fs::path& path) {
        fs::fstream file(path, fs::fstream::binary);
        if (!file.is_open()) return boost::none;

        object::Id root_id;

        boost::archive::text_oarchive oa(file);
        oa & root_id;

        return RootId(path, root_id);
    }

    RootId(const fs::path& file_path, const object::Id& root_id) :
        _file_path(file_path),
        _root_id(root_id)
    {
        store(root_id);
    }

    void store(const object::Id& root_id) {
        fs::fstream file(_file_path, fs::fstream::binary | fs::fstream::trunc | fs::fstream::out);
        if (!file.is_open())
            throw std::runtime_error("Failed to open root hash file");
        boost::archive::text_oarchive oa(file);
        oa & root_id;
        _root_id = root_id;
    }

    const object::Id& get() const {
        return _root_id;
    }

    void set(const object::Id& id) {
        _root_id = id;
    }

private:
    fs::path _file_path;
    object::Id _root_id;
};

BlockStore::BlockStore(const fs::path& basedir, unique_ptr<BlockSync> sync) :
    _rootdir(basedir / "blocks"),
    _objdir(basedir / "objects"),
    _sync(move(sync))
{
    fs::create_directories(_rootdir);
    fs::create_directories(_objdir);

    auto root_id_path = basedir / "root";
    auto opt_root_id = RootId::load(root_id_path);

    if (!opt_root_id) {
        object::Tree root_obj;
        opt_root_id = RootId(root_id_path, root_obj.store(_objdir));
    }

    _root_id = std::make_unique<RootId>(std::move(*opt_root_id));
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

void list(const fs::path& objdir, object::Id id, std::string pad = "") {
    auto obj = object::load<object::Tree, object::Block>(objdir, id);

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

template<class PathRange>
static
object::Id
_store(const fs::path& objdir,
        Opt<object::Id> old_object_id,
        PathRange path_range,
        const Data &data)
{
    auto on_exit = defer([&] {
            if (old_object_id) object::remove(objdir, *old_object_id);
        });

    if (path_range.empty()) {
        const object::Block block(data);
        return block.store(objdir);
    }

    auto child_name = path_range.front().string();
    path_range.advance_begin(1);

    object::Tree tree;

    if (old_object_id) {
        tree = object::load<object::Tree>(objdir, *old_object_id);
    }

    auto [child_i, inserted] = tree.insert(std::make_pair(child_name, object::Id()));

    Opt<object::Id> old_child_id;
    if (!inserted) old_child_id = child_i->second;

    child_i->second = _store(objdir, old_child_id, path_range, data);

    return tree.store(objdir);
}

void BlockStore::store(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto filepath = _get_data_file_path(block_id);

    {
        auto abs_filepath = _rootdir/filepath;
        fs::create_directories(abs_filepath.parent_path());
        data.StoreToFile(abs_filepath);
    }

    {
        auto dirs_range = boost::make_iterator_range(filepath);
        assert(!dirs_range.empty());

        auto id = _store(_objdir, _root_id->get(), dirs_range, data);
        _root_id->set(id);

        std::cerr << "Root: " << to_hex<char>(id) << "\n";
        list(_objdir, _root_id->get());
        std::cerr << "\n\n\n";
    }

    //_sync->add_action(BlockSync::ActionModifyBlock{block_id, digest});
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

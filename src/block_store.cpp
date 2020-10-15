#include "block_store.h"
#include "array_io.h"
#include "hex.h"
#include "version_vector.h"

#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <boost/filesystem.hpp>

#include <boost/archive/text_oarchive.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

#include <cpp-utils/system/diskspace.h>

#include "object/block.h"
#include "object/tree.h"
#include "object/io.h"

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

class ouisync::Root {
public:
    static Root load_or_create(const fs::path& rootdir, const fs::path& objdir, UserId user_id) {
        object::Id root_id;
        VersionVector clock;

        fs::path path = rootdir / user_id.to_string();

        fs::fstream file(path, file.binary | file.in);

        if (!file.is_open()) {
            object::Tree root_obj;
            root_id = root_obj.store(objdir);
            Root root{path, user_id, root_id, std::move(clock)};
            root.store();
            return root;
        }

        boost::archive::text_iarchive oa(file);
        object::tagged::Load<object::Id> load{root_id};
        oa >> load;
        oa >> clock;

        return Root{path, user_id, root_id, move(clock)};
    }

    void store() {
        fs::fstream file(_file_path, file.binary | file.trunc | file.out);
        if (!file.is_open())
            throw std::runtime_error("Failed to open root file");
        boost::archive::text_oarchive oa(file);
        object::tagged::Save<object::Id> save{_root_id};
        oa << save;
        oa << _clock;
    }

    const object::Id& get_id() const {
        return _root_id;
    }

    void set_id(const object::Id& id) {
        auto old_id = _root_id;
        _root_id = id;
        if (_root_id != old_id) {
            _clock.increment(_user_id);
            store();
        }
    }

    Root(const fs::path& file_path, const UserId& user_id, const object::Id& root_id, VersionVector clock) :
        _file_path(file_path), _user_id(user_id), _root_id(root_id), _clock(std::move(clock)) {}

private:
    fs::path _file_path;
    UserId _user_id;
    object::Id _root_id;
    VersionVector _clock;
};

BlockStore::BlockStore(const fs::path& basedir) :
    _rootdir(basedir / "roots"),
    _objdir(basedir / "objects")
{
    fs::create_directories(_rootdir);
    fs::create_directories(_objdir);

    _user_id = UserId::load_or_create(basedir / "user_id");
    _root = std::make_unique<Root>(Root::load_or_create(_rootdir, _objdir, _user_id));
}

bool BlockStore::tryCreate(const BlockId &block_id, const Data &data) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto filepath = _get_data_file_path(block_id);

    object::Block block(data);
    auto id = object::io::maybe_store(_objdir, _root->get_id(), filepath, block);

    if (id) {
        _root->set_id(*id);
        return true;
    }

    return false;
}

bool BlockStore::remove(const BlockId &block_id) {
    std::scoped_lock<std::mutex> lock(_mutex);

    auto opt_new_id = object::io::remove(_objdir, _root->get_id(), _get_data_file_path(block_id));
    if (!opt_new_id) return false;
    _root->set_id(*opt_new_id);

    return true;
}

optional<Data> BlockStore::load(const BlockId &block_id) const {
    std::scoped_lock<std::mutex> lock(const_cast<std::mutex&>(_mutex));
    try {
        auto path = _get_data_file_path(block_id);
        auto block = object::io::load(_objdir, _root->get_id(), path);
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
    auto id = object::io::store(_objdir, _root->get_id(), filepath, block);
    _root->set_id(id);
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
    HexBlockId hex_block_id; // just allocation on the stack
    return _for_each_block(objdir, id, f, hex_block_id, 0);
}

uint64_t BlockStore::numBlocks() const {
    uint64_t count = 0;
    _for_each_block(_objdir, _root->get_id(), [&] (const auto&) { ++count; });
    return count;
}

uint64_t BlockStore::estimateNumFreeBytes() const {
	return cpputils::free_disk_space_in_bytes(_objdir);
}

uint64_t BlockStore::blockSizeFromPhysicalBlockSize(uint64_t blockSize) const {
    return blockSize;
}

void BlockStore::forEachBlock(std::function<void (const BlockId &)> callback) const {
    _for_each_block(_objdir, _root->get_id(), callback);
}

BlockStore::~BlockStore() {}

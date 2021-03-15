#include "blob.h"
#include "block_store.h"
#include "hash.h"
#include "transaction.h"
#include "variant.h"

#include <boost/endian/conversion.hpp>
#include <array>
#include <iostream>

namespace endian = boost::endian;
using namespace ouisync;
using std::move;
using std::make_unique;
using std::cerr;

constexpr size_t BLOCK_SIZE = 1 << 15; // 32768

// TODO: Implement Left-Max-Data Tree

enum class BlockType { Node, Data };

static uint8_t type_to_byte(BlockType t) {
    switch (t) { case BlockType::Node: return 0u; case BlockType::Data: return 1u; }
}

static Opt<BlockType> byte_to_type(uint8_t b) {
    if (b == 0u) return BlockType::Node;
    if (b == 1u) return BlockType::Data;
    return boost::none;
}

struct NodeBlock : std::vector<ObjectId> {
    static constexpr size_t header_size = 1 /* BlockType */ + sizeof(uint16_t) /* size */;
    static constexpr size_t max_hash_count = (BLOCK_SIZE - header_size) / ObjectId::size;

    NodeBlock() {}

    explicit NodeBlock(const Block& b)
    {
        auto type = byte_to_type(b[0]);
        ouisync_assert(type == BlockType::Node);
        auto size = endian::big_to_native(reinterpret_cast<const uint16_t&>(b[1]));
        resize(size);

        const char* p = b.data() + header_size;

        for (auto& id : *this) {
            id.from_bytes(p);
            p += ObjectId::size;
        }
    }

    ObjectId calculate_id() const
    {
        // XXX: Very inefficient
        return BlockStore::calculate_block_id(to_block());
    }

    Block to_block() const {
        ouisync_assert(size() <= max_hash_count);
        Block r(BLOCK_SIZE);
        r[0] = type_to_byte(BlockType::Node);

        reinterpret_cast<uint16_t&>(r[1]) = endian::native_to_big(uint16_t(size()));

        auto p = &r[0] + header_size;

        for (auto& id : *this) {
            id.to_bytes(p);
            p += ObjectId::size;
        }

        return r;
    }
};

struct DataBlock : public Block {
    static constexpr size_t header_size = 1 /* BlockType */ + sizeof(uint16_t) /* size */;
    static constexpr size_t max_data_size = BLOCK_SIZE - header_size;

    DataBlock() = default;
    DataBlock(Block&& b) : Block(move(b)) {}

    static void initialize_empty(Block& b)
    {
        b.resize(BLOCK_SIZE, 0);
        b[0] = type_to_byte(BlockType::Data);
        reinterpret_cast<uint16_t&>(b[1]) = 0u;
    }

    ObjectId calculate_id()
    {
        if (empty()) initialize_empty(*this);
        ouisync_assert(size() == BLOCK_SIZE);
        return BlockStore::calculate_block_id(data(), BLOCK_SIZE);
    }

    uint16_t data_size() const {
        if (empty()) return 0;
        ouisync_assert(size() >= header_size);
        if (size() < header_size) return 0;
        auto ret = reinterpret_cast<const uint16_t&>((*this)[1]);
        return endian::big_to_native(ret);
    }

    void data_resize(uint16_t s) {
        ouisync_assert(s <= max_data_size);
        if (empty()) initialize_empty(*this);
        reinterpret_cast<uint16_t&>((*this)[1]) = endian::native_to_big(s);
    }

    size_t read(char* buffer, size_t size, size_t offset) const
    {
        if (empty()) return 0;
        auto len = data_size();
        if (offset >= len) return 0;
        if (offset + size > len) size = len - offset;
        std::copy_n(data() + offset + header_size, size, buffer);
        return size;
    }

    size_t write(const char* buffer, size_t size, size_t offset)
    {
        if (offset >= max_data_size) {
            return 0;
        }

        if (offset + size > max_data_size) {
            size = max_data_size - offset;
        }

        size_t len = data_size();

        if (offset + size > len) {
            data_resize(offset + size);
        }

        std::copy_n(buffer, size, data() + header_size + offset);

        return size;
    }
};

struct Blob::Impl
{
    // The block_store variable may be nullptr if this Blob was initially
    // empty.
    const BlockStore* block_store;
    // If top_block_id is boost::none, then recalculation is needed.
    Opt<ObjectId> top_block_id;
    variant<NodeBlock, DataBlock> top_block;
    std::unordered_map<ObjectId, DataBlock> blocks;

    ObjectId maybe_calculate_id()
    {
        if (top_block_id) return *top_block_id;

        ouisync::apply(top_block,
            [&] (auto& b) { top_block_id = b.calculate_id(); });

        return *top_block_id;
    }

    size_t read(char* buffer, size_t size, size_t offset)
    {
        return ouisync::apply(top_block,
                [&] (const auto& b) { return read(b, buffer, size, offset); });

    }

    size_t read(const NodeBlock& b, char* buffer, size_t size, size_t offset)
    {
        size_t read = 0;

        size_t i = offset / DataBlock::max_data_size;

        while (read < size) {
            if (i >= b.size()) return read;
            auto id = b[i];

            DataBlock tmp;
            const DataBlock* p = load_const_data_block(id, tmp);

            auto r = p->read(buffer, size - read, offset);

            read   += r;
            buffer += r;
            offset  = 0;
            ++i;
        }

        return read;
    }

    size_t read(const DataBlock& b, char* buffer, size_t size, size_t offset)
    {
        return b.read(buffer, size, offset);
    }

    size_t write(const char* buffer, size_t size, size_t offset)
    {
        return ouisync::apply(top_block,
                [&] (auto& b) { return write(b, buffer, size, offset); });
    }

    size_t write(NodeBlock& node, const char* buffer, size_t size, size_t offset)
    {
        if (size != 0) top_block_id = boost::none;

        const auto m = DataBlock::max_data_size;
        size_t wrote = 0;

        while (wrote < size) {
            auto i = offset / m;

            DataBlock block;

            ouisync_assert(i <= node.size());

            if (i < node.size()) {
                block = remove_data_block(node[i]);
            }

            auto r = block.write(buffer, size - wrote, offset - i*m);

            wrote  += r;
            buffer += r;
            offset += r;

            auto id = block.calculate_id();
            blocks.insert({id, move(block)});

            if (i < node.size()) {
                node[i] = id;
            } else {
                node.push_back(id);
            }
        }

        return wrote;
    }

    size_t write(DataBlock& b, const char* buffer, size_t size, size_t offset)
    {
        constexpr auto m = DataBlock::max_data_size;

        if (size == 0) return 0;

        top_block_id = boost::none;

        if (offset + size <= m) {
            // It fits, we're done
            return b.write(buffer, size, offset);
        }

        // We'll replace the top object with a node, make this data block it's
        // first block, and finally append whatever number of blocks is still
        // needed.

        NodeBlock n;

        size_t wrote = 0;

        while (wrote < size) {
            auto r = b.write(buffer, size - wrote, offset);
            buffer += r;
            wrote  += r;
            offset = 0;

            auto id = b.calculate_id();
            n.push_back(id);
            blocks.insert({id, move(b)});
        }

        top_block = move(n);

        return wrote;
    }

    size_t truncate(size_t s) {
        return ouisync::apply(top_block, [&] (auto& b) { return truncate(b, s); });
    }

    size_t truncate(DataBlock& b, size_t s) {
        s = std::min<size_t>(b.data_size(), s);
        if (b.data_size() == s) return s;
        top_block_id = boost::none;
        b.data_resize(s);
        return s;
    }

    size_t truncate(NodeBlock& node, size_t new_data_size) {
        if (node.empty()) return 0;

        constexpr auto m = DataBlock::max_data_size;

        if (m == 0) return 0;
        size_t new_node_size = std::min((new_data_size+m-1) / m, node.size());

        // Remove children with index new_node_size and higner
        for (size_t i = 0; i < node.size() - new_node_size; ++i) {
            blocks.erase(node[new_node_size + i]);
        }

        node.resize(new_node_size);

        if (node.size() == 0) {
            top_block = DataBlock();
            return 0;
        }

        DataBlock last = remove_data_block(node.back());

        size_t s = std::min<size_t>(new_data_size - (new_node_size - 1) * m, last.data_size());
        new_data_size = m * (node.size() - 1) + s;

        if (s == last.data_size()) {
            if (new_node_size == 1) {
                top_block = move(last);
            } else {
                blocks.insert({node.back(), move(last)});
            }
            return new_data_size;
        }

        last.data_resize(s);

        if (new_node_size == 1) {
            top_block = move(last);
        } else {
            auto id = last.calculate_id();
            node.back() = id;
            blocks.insert({id, move(last)});
        }

        return new_data_size;
    }

    size_t size() const {
        return ouisync::apply(top_block,
            [&] (const DataBlock& b) -> size_t { return b.data_size(); },
            [&] (const NodeBlock& n) -> size_t {
                if (n.size() == 0) return 0;

                DataBlock tmp;
                const DataBlock* last = load_const_data_block(n.back(), tmp);

                // Assuming all except the last block are filled to the
                // max_data_size.
                return (n.size()-1) * DataBlock::max_data_size + last->data_size();
            });
    }

    void commit(Transaction& tnx) {
        auto top_id = maybe_calculate_id();

        apply(top_block,
            [&] (DataBlock& b) {
                tnx.insert_block(top_id, move(b));
            },
            [&] (NodeBlock& n) {
                tnx.insert_block(top_id, n.to_block());
                for (auto& id : n) {
                    tnx.insert_edge(top_id, id);
                    tnx.insert_block(id, move(blocks.at(id)));
                }
            });
    }

    const DataBlock* load_const_data_block(const ObjectId& id, DataBlock& stack_tmp) const
    {
        auto i = blocks.find(id);

        if (i != blocks.end()) {
            return &i->second;
        } else {
            stack_tmp = DataBlock{block_store->load(id)};
            return &stack_tmp;
        }
    }

    DataBlock remove_data_block(const ObjectId& id)
    {
        auto i = blocks.find(id);

        if (i != blocks.end()) {
            DataBlock ret = move(i->second);
            blocks.erase(i);
            return ret;
        } else {
            return DataBlock{block_store->load(id)};
        }
    }
};

void Blob::maybe_init()
{
    if (_impl) return;
    _impl = make_unique<Impl>();
    _impl->top_block = DataBlock();
}

size_t Blob::read(char* buffer, size_t size, size_t offset)
{
    if (!_impl) return 0;
    return _impl->read(buffer, size, offset);
}

size_t Blob::write(const char* buffer, size_t size, size_t offset)
{
    maybe_init();
    return _impl->write(buffer, size, offset);
}

size_t Blob::truncate(size_t size)
{
    if (!_impl) return 0;
    return _impl->truncate(size);
}

void Blob::commit(Transaction& transaction)
{
    maybe_init();
    _impl->commit(transaction);
    _impl = nullptr;
}

static
variant<NodeBlock, DataBlock> typed_block(Block&& b) {
    auto opt_type = byte_to_type(b[0]);
    if (!opt_type) throw std::runtime_error("Block of unknonw type");
    switch (*opt_type) {
        case BlockType::Node: return NodeBlock(move(b));
        case BlockType::Data: return DataBlock{move(b)};
    }
}

/* static */
Blob Blob::open(const ObjectId& id, const BlockStore& block_store)
{
    auto block = block_store.load(id);

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->top_block_id = id;
    impl->top_block = typed_block(move(block));

    return {move(impl)};
}

/* static */
Blob Blob::open(const fs::path& path, const BlockStore& block_store)
{
    auto block = block_store.load(path);

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->top_block = typed_block(move(block));

    return {move(impl)};
}

/* static */
Opt<Blob> Blob::maybe_open(const ObjectId& id, const BlockStore& block_store)
{
    auto block = block_store.maybe_load(id);

    if (!block) return boost::none;

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->top_block_id = id;
    impl->top_block = typed_block(move(*block));

    return {move(impl)};
}

size_t Blob::size() const
{
    if (!_impl) return 0;
    return _impl->size();
}

Blob::Blob() {}
Blob::Blob(std::unique_ptr<Impl> impl) : _impl(std::move(impl)) {}
Blob::~Blob() {}

Blob::Blob(Blob&& other) :
    _impl(move(other._impl))
{}

Blob& Blob::operator=(Blob&& other)
{
    _impl = move(other._impl);
    return *this;
}

ObjectId Blob::id()
{
    maybe_init();
    return _impl->maybe_calculate_id();
}

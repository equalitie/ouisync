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

class NodeBlock {
public:
    static_assert(BlockId::size == sizeof(BlockId));

    static constexpr size_t header_size = 1 /* BlockType */ + sizeof(uint16_t) /* size */;
    static constexpr size_t max_hash_count = (BLOCK_SIZE - header_size) / BlockId::size;

    NodeBlock() {}

    explicit NodeBlock(Block&& b) : _block(move(b))
    {
        ouisync_assert(_block.empty() || _block.size() == BLOCK_SIZE);
        ouisync_assert(byte_to_type(_block[0]) == BlockType::Node);
    }

    const BlockId& operator[](size_t i) const {
        ouisync_assert(i < max_hash_count);
        ouisync_assert(i < size());
        return *(begin() + i);
    }

    BlockId& operator[](size_t i) {
        ouisync_assert(i < max_hash_count);
        ouisync_assert(i < size());
        return *(begin() + i);
    }

    void push_back(const BlockId& id) {
        auto s = size();
        ouisync_assert(s < max_hash_count);
        resize(s+1);
        (*this)[s] = id;
    }

    uint16_t size() const {
        if (_block.size() < header_size) return 0;
        return endian::big_to_native(
                reinterpret_cast<const uint16_t&>(_block[1]));
    }

    void resize(uint16_t s) {
        ouisync_assert(s < max_hash_count);
        if (_block.size() < header_size) {
            _block.resize(BLOCK_SIZE, 0);
            _block[0] = type_to_byte(BlockType::Node);
        }
        reinterpret_cast<uint16_t&>(_block[1]) = endian::native_to_big(s);
    }

    BlockId calculate_id() const
    {
        return BlockStore::calculate_block_id(_block);
    }

    Block& as_block() { return _block; }

    BlockId& back() {
        ouisync_assert(!empty());
        return (*this)[size() - 1];
    }

    const BlockId& back() const {
        ouisync_assert(!empty());
        return (*this)[size() - 1];
    }

    bool empty() const { return size() == 0; }

    const BlockId* begin() const {
        if (empty()) return nullptr;
        return reinterpret_cast<const BlockId*>(&_block[header_size]);
    }

    const BlockId* end() const {
        if (empty()) return nullptr;
        return reinterpret_cast<const BlockId*>(
                &_block[header_size + size() * BlockId::size]);
    }

    BlockId* begin() {
        if (empty()) return nullptr;
        return reinterpret_cast<BlockId*>(&_block[header_size]);
    }

    BlockId* end() {
        if (empty()) return nullptr;
        return reinterpret_cast<BlockId*>(
                &_block[header_size + size() * BlockId::size]);
    }

private:
    Block _block;
};

class DataBlock {
private:
    static constexpr size_t header_size = 1 /* BlockType */ + sizeof(uint16_t) /* size */;

public:
    static constexpr size_t max_data_size = BLOCK_SIZE - header_size;

    DataBlock() = default;
    DataBlock(Block&& b) : _block(move(b)) {}

    BlockId calculate_id()
    {
        if (_block.empty()) initialize_empty(_block);
        ouisync_assert(_block.size() == BLOCK_SIZE);
        return BlockStore::calculate_block_id(_block.data(), BLOCK_SIZE);
    }

    uint16_t data_size() const {
        if (_block.empty()) return 0;
        ouisync_assert(_block.size() >= header_size);
        if (_block.size() < header_size) return 0;
        auto ret = reinterpret_cast<const uint16_t&>(_block[1]);
        return endian::big_to_native(ret);
    }

    void data_resize(uint16_t s) {
        ouisync_assert(s <= max_data_size);
        if (_block.empty()) initialize_empty(_block);
        reinterpret_cast<uint16_t&>(_block[1]) = endian::native_to_big(s);
    }

    size_t read(char* buffer, size_t size, size_t offset) const
    {
        if (_block.empty()) return 0;
        auto len = data_size();
        if (offset >= len) return 0;
        if (offset + size > len) size = len - offset;
        std::copy_n(_block.data() + offset + header_size, size, buffer);
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

        std::copy_n(buffer, size, _block.data() + header_size + offset);

        return size;
    }

    Block& as_block() { return _block; }

private:
    static void initialize_empty(Block& b)
    {
        b.resize(BLOCK_SIZE, 0);
        b[0] = type_to_byte(BlockType::Data);
        reinterpret_cast<uint16_t&>(b[1]) = 0u;
    }

private:
    Block _block;
};

struct Blob::Impl
{
    // The block_store variable may be nullptr if this Blob was initially
    // empty.
    const BlockStore* block_store;
    // If top_block_id is boost::none, then recalculation is needed.
    Opt<BlockId> top_block_id;
    variant<NodeBlock, DataBlock> top_block;
    std::unordered_map<BlockId, DataBlock> blocks;

    BlockId maybe_calculate_id()
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

    size_t read(const NodeBlock& node, char* buffer, size_t size, size_t offset)
    {
        size_t read = 0;

        size_t i = offset / DataBlock::max_data_size;

        while (read < size) {
            if (i >= node.size()) return read;
            auto id = node[i];

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

        // We'll replace the top DataBlock with a NodeBlock and make the data
        // block first block of the node. Finally, we'll append whatever number
        // of blocks is still needed.

        NodeBlock node;

        size_t wrote = 0;

        while (wrote < size) {
            auto r = b.write(buffer, size - wrote, offset);
            buffer += r;
            wrote  += r;
            offset = 0;

            auto id = b.calculate_id();
            node.push_back(id);
            blocks.insert({id, move(b)});
        }

        top_block = move(node);

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
        size_t new_node_size = std::min<uint16_t>((new_data_size+m-1) / m, node.size());

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
            [&] (const DataBlock& b) -> size_t {
                return size(b);
            },
            [&] (const NodeBlock& n) -> size_t {
                auto s = size(n);
                return s;
            });
    }

    size_t size(const DataBlock& b) const {
        return b.data_size();
    }

    size_t size(const NodeBlock& n) const {
        if (n.size() == 0) return 0;

        DataBlock tmp;
        const DataBlock* last = load_const_data_block(n.back(), tmp);

        // Assuming all except the last block are filled to the
        // max_data_size.
        return (n.size()-1) * DataBlock::max_data_size + last->data_size();
    }

    BlockId commit(Transaction& tnx) {
        auto top_id = maybe_calculate_id();

        apply(top_block,
            [&] (DataBlock& b) {
                tnx.insert_block(top_id, move(b.as_block()));
            },
            [&] (NodeBlock& n) {
                ouisync_assert(n.size());

                for (auto& id : n) {
                    auto i = blocks.find(id);
                    // Those not in blocks are assumed to not have been
                    // modified.
                    if (i == blocks.end()) continue;

                    tnx.insert_edge(top_id, id);
                    tnx.insert_block(id, move(i->second.as_block()));
                    blocks.erase(i);
                }

                tnx.insert_block(top_id, move(n.as_block()));

                ouisync_assert(blocks.empty());
            });

        return top_id;
    }

    const DataBlock* load_const_data_block(const BlockId& id, DataBlock& stack_tmp) const
    {
        auto i = blocks.find(id);

        if (i != blocks.end()) {
            return &i->second;
        } else {
            stack_tmp = DataBlock{block_store->load(id)};
            return &stack_tmp;
        }
    }

    DataBlock remove_data_block(const BlockId& id)
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

BlockId Blob::commit(Transaction& transaction)
{
    maybe_init();
    auto id = _impl->commit(transaction);
    _impl = nullptr;
    return id;
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
Blob Blob::open(const BlockId& id, const BlockStore& block_store)
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
Opt<Blob> Blob::maybe_open(const BlockId& id, const BlockStore& block_store)
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

BlockId Blob::id()
{
    maybe_init();
    return _impl->maybe_calculate_id();
}

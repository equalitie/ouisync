#include "blob.h"
#include "block_store.h"
#include "hash.h"
#include "transaction.h"

using namespace ouisync;
using std::move;
using std::make_unique;

struct Blob::Impl 
{
    const BlockStore* block_store;
    Opt<ObjectId> block_id;
    // TODO: Implement Left-Max-Data Tree
    Block block;

    ObjectId maybe_calculate_id()
    {
        if (block_id) return *block_id;
        Sha256 hash;
        hash.update(block.data(), block.size());
        block_id = hash.close();
        return *block_id;
    }

    size_t read(char* buffer, size_t size, size_t offset)
    {
        size_t len = block.size();

        if (offset < len) {
            if (offset + size > len) size = len - offset;
            memcpy((void*)buffer, block.data() + offset, size);
        } else {
            size = 0;
        }

        return size;
    }

    size_t write(const char* buffer, size_t size, size_t offset)
    {
        if (size == 0) return 0;

        block_id = boost::none;

        size_t len = block.size();

        if (offset + size > len) {
            block.resize(offset + size);
        }

        memcpy(block.data() + offset, buffer, size);

        return size;
    }

    size_t truncate(size_t s) {
        if (block.size() == s) return s;
        block.resize(std::min<size_t>(block.size(), s));
        block_id = boost::none;
        return block.size();
    }

    size_t size() const { return block.size(); }

    void commit(Transaction& tnx) {
        auto id = maybe_calculate_id();
        tnx.insert_block(id, block);
    }
};

size_t Blob::read(char* buffer, size_t size, size_t offset)
{
    return _impl->read(buffer, size, offset);
}

size_t Blob::write(const char* buffer, size_t size, size_t offset)
{
    return _impl->write(buffer, size, offset);
}

size_t Blob::truncate(size_t size)
{
    return _impl->truncate(size);
}

void Blob::commit(Transaction& transaction)
{
    _impl->commit(transaction);
}

/* static */
Blob Blob::empty()
{
    auto impl = make_unique<Impl>();
    impl->block_store = nullptr;
    return {move(impl)};
}

/* static */
Blob Blob::open(const ObjectId& id, const BlockStore& block_store)
{
    auto block = block_store.load(id);

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->block_id = id;
    impl->block = move(block);

    return {move(impl)};
}

/* static */
Blob Blob::open(const fs::path& path, const BlockStore& block_store)
{
    auto block = block_store.load(path);

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->block = move(block);

    return {move(impl)};
}

/* static */
Opt<Blob> Blob::maybe_open(const ObjectId& id, const BlockStore& block_store)
{
    auto block = block_store.maybe_load(id);

    if (!block) return boost::none;

    auto impl = make_unique<Impl>();

    impl->block_store = &block_store;
    impl->block_id = id;
    impl->block = move(*block);

    return {move(impl)};
}

size_t Blob::size() const
{
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
    return _impl->maybe_calculate_id();
}

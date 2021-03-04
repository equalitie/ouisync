#include "file.h"
#include "hash.h"
#include "archive.h"
#include "object_tag.h"
#include "blob.h"
#include "block_store.h"

#include <sstream>
#include <boost/optional.hpp>

#include <boost/serialization/array_wrapper.hpp>

using namespace ouisync;

ObjectId File::calculate_id() const
{
    // XXX: This is inefficient
    std::stringstream ss;
    auto array = boost::serialization::make_array(data(), size());
    auto tag = ObjectTag::File;
    archive::store(ss, tag, uint32_t(size()), array);
    return BlockStore::calculate_block_id(ss.str().data(), ss.str().size());
}

ObjectId File::save(BlockStore& blockstore) const
{
    // XXX: This is inefficient
    std::stringstream ss;
    auto array = boost::serialization::make_array(data(), size());
    auto tag = ObjectTag::File;
    archive::store(ss, tag, uint32_t(size()), array);
    return blockstore.store(ss.str().data(), ss.str().size());
}

bool File::maybe_load(Blob& blob)
{
    BlobStreamBuffer buf(blob);
    std::istream s(&buf);
    InputArchive a(s);
    ObjectTag tag;
    a >> tag;
    if (tag != ObjectTag::File) return false;
    uint32_t size;
    a >> size;
    resize(size);
    auto array = boost::serialization::make_array(data(), this->size());
    a >> array;
    return true;
}

/* static */
size_t File::read_size(Blob& blob)
{
    BlobStreamBuffer buf(blob);
    std::istream s(&buf);
    InputArchive a(s);
    ObjectTag tag;
    a >> tag;
    if (tag != ObjectTag::File) throw std::runtime_error("Block doesn't represent a file");
    uint32_t size;
    a >> size;
    return size;
}

std::ostream& ouisync::operator<<(std::ostream& os, const File& b) {
    auto id = b.calculate_id();
    return os << "Data id:" << id << " size:" << b.size();
}


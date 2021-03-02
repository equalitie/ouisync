#include "file_blob.h"
#include "hash.h"
#include "archive.h"
#include "object_tag.h"

#include <sstream>
#include <boost/optional.hpp>

#include <boost/serialization/array_wrapper.hpp>

using namespace ouisync;

ObjectId FileBlob::calculate_id() const
{
    // XXX: This is inefficient
    std::stringstream ss;
    auto array = boost::serialization::make_array(data(), size());
    auto tag = ObjectTag::FileBlob;
    archive::store(ss, tag, uint32_t(size()), array);
    return BlockStore::calculate_block_id(ss.str().data(), ss.str().size());
}

ObjectId FileBlob::save(BlockStore& blockstore) const
{
    // XXX: This is inefficient
    std::stringstream ss;
    auto array = boost::serialization::make_array(data(), size());
    auto tag = ObjectTag::FileBlob;
    archive::store(ss, tag, uint32_t(size()), array);
    return blockstore.store(ss.str().data(), ss.str().size());
}

bool FileBlob::maybe_load(const BlockStore::Block& block)
{
    // XXX: This is inefficient
    std::stringstream ss;
    ss.str(std::string(block.data(), block.size()));
    ObjectTag tag;
    InputArchive a(ss);
    a >> tag;
    if (tag != ObjectTag::FileBlob) return false;
    uint32_t s;
    a >> s;
    resize(s);
    auto array = boost::serialization::make_array(data(), size());
    a >> array;
    return true;
}

/* static */
size_t FileBlob::read_size(const BlockStore::Block& block)
{
    // XXX: This is inefficient
    std::stringstream ss;
    ss.str(std::string(block.data(), block.size()));
    ObjectTag tag;
    InputArchive a(ss);
    a >> tag;
    if (tag != ObjectTag::FileBlob) throw std::runtime_error("Block doesn't represent a file");
    uint32_t s;
    a >> s;
    return s;
}

std::ostream& ouisync::operator<<(std::ostream& os, const FileBlob& b) {
    auto id = b.calculate_id();
    return os << "Data id:" << id << " size:" << b.size();
}


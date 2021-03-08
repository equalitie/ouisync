#include "file.h"
#include "hash.h"
#include "archive.h"
#include "object_tag.h"
#include "blob.h"
#include "block_store.h"

#include <iostream>
#include <boost/optional.hpp>

#include <boost/serialization/array_wrapper.hpp>

using namespace ouisync;
using std::move;

File::File(BlockStore& block_store) :
    _blob(Blob::empty(block_store))
{
    BlobStreamBuffer buf(_blob);
    std::ostream stream(&buf);
    OutputArchive a(stream);

    a << ObjectTag::File;
    _data_offset = buf.getp();
}

File::File(Blob&& blob, size_t data_offset) :
    _blob(move(blob)),
    _data_offset(data_offset)
{
}

ObjectId File::calculate_id()
{
    return _blob.id();
}

/* static */
Opt<File> File::maybe_open(Blob& blob)
{
    BlobStreamBuffer buf(blob);
    std::istream stream(&buf);
    InputArchive a(stream);

    ObjectTag tag;
    a >> tag;
    if (tag != ObjectTag::File) return boost::none;

    size_t data_offset = buf.getg();

    return File{move(blob), data_offset};
}

size_t File::size()
{
    return _blob.size() - _data_offset;
}

size_t File::write(const char* buf, size_t size, size_t offset)
{
    return _blob.write(buf, size, _data_offset + offset);
}

size_t File::read(char* buf, size_t size, size_t offset)
{
    return _blob.read(buf, size, _data_offset + offset);
}

size_t File::truncate(size_t s)
{
    return _blob.truncate(s + _data_offset) - _data_offset;
}

void File::save(Transaction& tnx)
{
    _blob.commit(tnx);
}

std::ostream& ouisync::operator<<(std::ostream& os, const File& b) {
    auto id = const_cast<File&>(b).calculate_id();
    return os << "Data id:" << id << " size:" << b._blob.size();
}


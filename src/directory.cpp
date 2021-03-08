#include "directory.h"
#include "ostream/padding.h"
#include "archive.h"
#include "object_tag.h"
#include "blob.h"
#include "transaction.h"

#include <iostream>
#include <sstream>
#include <boost/filesystem.hpp>

#include <boost/serialization/map.hpp>

using namespace ouisync;

ObjectId Directory::calculate_id() const
{
    // XXX: This is inefficient
    std::stringstream ss;
    auto tag = ObjectTag::Directory;
    archive::store(ss, tag, _name_map);
    return BlockStore::calculate_block_id(ss.str().data(), ss.str().size());
}

ObjectId Directory::save(BlockStore& blockstore, Transaction& tnx) const
{
    auto blob = Blob::empty(blockstore);
    BlobStreamBuffer buf(blob);
    std::ostream s(&buf);
    OutputArchive a(s);
    a << ObjectTag::Directory;
    a << _name_map;
    blob.commit(tnx);
    auto id = blob.id();

    for_each_unique_child([&] (auto&, auto& child_id) { tnx.insert_edge(id, child_id); });

    return blob.id();
}

bool Directory::maybe_load(Blob& blob)
{
    BlobStreamBuffer buf(blob);
    std::istream s(&buf);
    InputArchive a(s);
    ObjectTag tag;
    a >> tag;
    if (tag != ObjectTag::Directory) return false;
    a >> _name_map;
    return true;
}

/* static */
bool Directory::blob_is_dir(Blob& blob)
{
    BlobStreamBuffer buf(blob);
    std::istream s(&buf);
    InputArchive a(s);
    ObjectTag tag;
    a >> tag;
    return tag == ObjectTag::Directory;
}

VersionVector Directory::calculate_version_vector_union() const
{
    VersionVector result;

    for (auto& [filename, user_map] : _name_map) {
        for (auto& [username, vobj] : user_map) {
            result = result.merge(vobj.versions);
        }
    }

    return result;
}

void Directory::print(std::ostream& os, unsigned level) const
{
    os << Padding(level*4) << "Directory id:" << calculate_id() << "\n";
    for (auto& [filename, user_map] : _name_map) {
        os << Padding(level*4) << "  filename:" << filename << "\n";
        for (auto& [user, vobj]: user_map) {
            os << Padding(level*4) << "    user:" << user << "\n";
            os << Padding(level*4) << "    obj:"  << vobj.id << "\n";
        }
    }
}

std::ostream& ouisync::operator<<(std::ostream& os, const Directory& tree) {
    tree.print(os, 0);
    return os;
}

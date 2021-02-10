#include "index.h"
#include "hash.h"
#include "ouisync_assert.h"
#include "object_store.h"
#include "directory.h"
#include "file_blob.h"
#include "variant.h"

using namespace ouisync;
using std::string;

Index::Index(ObjectStore& objstore) :
    _objstore(objstore)
{}

void Index::insert_object(const ObjectId& id, const string& filename, const ObjectId& parent_id)
{
    auto i = _elements.insert({id, {}}).first;
    Sha256 hash;
    hash.update(parent_id);
    hash.update(filename);
    auto inserted = i->second.insert(hash.close()).second;
    ouisync_assert(inserted);
}

void Index::remove_object(const ObjectId& id, const string& filename, const ObjectId& parent_id) {
    auto i = _elements.find(id);
    ouisync_assert(i != _elements.end());
    Sha256 hash;
    hash.update(parent_id);
    hash.update(filename);
    auto erased = i->second.erase(hash.close());
    ouisync_assert(erased);
    if (!i->second.empty()) return;

    // If we're here, that means no other node points to this object.
    _elements.erase(i);

    auto obj = _objstore.load<Directory, FileBlob::Nothing>(id);

    apply(obj,
            [&](const Directory& d) {
                d.for_each_unique_child([&] (auto& filename, auto& object_id) {
                    remove_object(object_id, filename, id);
                });
            },
            [&](const FileBlob::Nothing&) {
            });

    _objstore.remove(id);
}

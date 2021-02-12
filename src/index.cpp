#include "index.h"
#include "hash.h"
#include "ouisync_assert.h"
#include "object_store.h"
#include "directory.h"
#include "file_blob.h"
#include "variant.h"
#include "ouisync_assert.h"

#include <iostream>

using namespace ouisync;
using std::string;
using std::move;

Index::Element::Element(const ObjectId& id, const ObjectId& parent_id) :
    _is_root(id == parent_id),
    _obj_id(id),
    _parent_id(parent_id)
{
}

void Index::set_version_vector(const VersionVector& vv)
{
    ouisync_assert(vv.happened_after(_commit.stamp));

    if (!vv.happened_after(_commit.stamp)) return;
    if (vv == _commit.stamp) return;

    _commit.stamp = vv;
}

void Index::insert_object(const ObjectId& obj_id, const ObjectId& parent_id)
{
    auto i = _elements.insert({obj_id, {}}).first;
    auto& parents = i->second;
    auto j = parents.insert({parent_id, 0u}).first;
    j->second++;

    if (obj_id == parent_id) {
        _commit.root_id = obj_id;
    }
}

void Index::remove_object(const ObjectId& obj_id, const ObjectId& parent_id)
{
    auto i = _elements.find(obj_id);

    ouisync_assert(i != _elements.end());

    auto& parents = i->second;

    auto j = parents.find(parent_id);

    ouisync_assert(j != parents.end());
    ouisync_assert(j->second != 0u);

    if (--j->second == 0) {
        parents.erase(j);
    }

    if (parents.empty()) _elements.erase(i);
}

bool Index::has(const ObjectId& obj_id) const
{
    auto i = _elements.find(obj_id);
    return i != _elements.end();
}

bool Index::has(const ObjectId& obj_id, const ObjectId& parent_id) const
{
    auto i = _elements.find(obj_id);
    if (i == _elements.end()) return false;
    auto j = i->second.find(parent_id);
    if (j == i->second.end()) return false;
    ouisync_assert(j->second != 0);
    return j->second != 0;
}

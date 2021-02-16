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

Index::Index()
    : _commit({{}, Directory{}.calculate_id()})
{
    insert_object(_commit.root_id, _commit.root_id);
}

void Index::set_version_vector(const VersionVector& vv)
{
    ouisync_assert(vv.happened_after(_commit.stamp));

    if (!vv.happened_after(_commit.stamp)) return;
    if (vv == _commit.stamp) return;

    _commit.stamp = vv;
}

void Index::insert_object(const ObjectId& obj_id, const ObjectId& parent_id, size_t cnt)
{
    if (cnt == 0) return;

    auto i = _elements.insert({obj_id, {}}).first;
    auto& parents = i->second;
    auto j = parents.insert({parent_id, 0u}).first;
    j->second += cnt;

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

size_t Index::count_object_in_parent(const ObjectId& obj_id, const ObjectId& parent_id) const
{
    auto i = _elements.find(obj_id);
    if (i == _elements.end()) return 0;
    auto j = i->second.find(parent_id);
    if (j == i->second.end()) return 0;
    ouisync_assert(j->second != 0);
    return j->second != 0;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Index& index)
{
    os << index._commit << "\n";
    for (auto& [obj_id, parents]: index._elements) {
        os << "  " << obj_id << "\n";
        for (auto& [parent_id, cnt] : parents) {
            os << "    " << parent_id << " " << cnt << "\n";
        }
    }
    return os;
}

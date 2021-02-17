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

Index::Index(const UserId& user)
{
    _user_states.insert({user, {}});
}

Index::UserState::UserState() : 
    commit({{}, Directory{}.calculate_id()})
{
    Index::insert_object(*this, commit.root_id, commit.root_id, 1);
}

void Index::set_version_vector(const UserId& user, const VersionVector& vv)
{
    auto i = _user_states.find(user);
    ouisync_assert(i != _user_states.end());
    auto& state = i->second;
    ouisync_assert(vv.happened_after(state.commit.stamp));

    if (!vv.happened_after(state.commit.stamp)) return;
    if (vv == state.commit.stamp) return;

    state.commit.stamp = vv;
}

template<class F> void Index::remove_count(Elements& elements, F&& f) {
    for (auto i = elements.begin(); i != elements.end();) {
        auto i_next = std::next(i);
        auto& parents = i->second;

        for (auto j = parents.begin(); j != parents.end(); ) {
            auto j_next = std::next(j);
            size_t cnt = f(i->first, j->first, j->second);
            ouisync_assert(cnt <= j->second);
            j->second -= cnt;
            if (j->second == 0) { parents.erase(j); }
            j = j_next;
        }

        if (parents.empty()) elements.erase(i);

        i = i_next;
    }
}

template<class F> void Index::for_each(const Elements& elements, F&& f) {
    for (auto i = elements.begin(); i != elements.end(); ++i) {
        auto& parents = i->second;

        for (auto j = parents.begin(); j != parents.end(); ++j) {
            f(i->first, j->first, j->second);
        }
    }
}

void Index::merge(const Index& other, ObjectStore& objstore)
{
    for (auto& [user, remote_] : other._user_states) {
        auto& remote = remote_; // because clang complains when capturing the above in a lambda :-/
        auto& local = _user_states.insert({user, {}}).first->second;

        if (local.commit.happened_after(remote.commit)) return;
        if (local.commit == remote.commit) return;

        // One user will never/must not produce concurrent commits.
        ouisync_assert(local.commit.happened_before(remote.commit));

        // XXX: The two operations below can be merged to have it
        // done in linear time.

        std::set<ObjectId> possibly_remove_from_objstore;

        // Remove elements from local.elements that are not in the new one
        remove_count(local.elements, [&] (auto& obj_id, auto& parent_id, auto old_cnt) mutable -> size_t {
                auto new_cnt = count_object_in_parent(remote.elements, obj_id, parent_id);
                if (new_cnt >= old_cnt) return 0;
                auto remove_cnt = old_cnt - new_cnt;
                if (new_cnt == 0) {
                    possibly_remove_from_objstore.insert(obj_id);
                }
                return remove_cnt;
            });

        for (auto& obj : possibly_remove_from_objstore) {
            if (someone_has(obj)) continue;
            objstore.remove(obj);
            _missing_objects.erase(obj);
        }

        for_each(remote.elements, [&] (auto& obj_id, auto& parent_id, auto new_cnt) {
                auto old_cnt = count_object_in_parent(local.elements, obj_id, parent_id);
                ouisync_assert(old_cnt <= new_cnt);
                if (old_cnt == new_cnt) return;
                insert_object(local, obj_id, parent_id, new_cnt - old_cnt);
                if (old_cnt == 0) {
                    if (!objstore.exists(obj_id)) {
                        _missing_objects.insert(obj_id);
                    }
                }
            });

        local.commit.stamp = remote.commit.stamp;
    }
}

void Index::insert_object(const UserId& user, const ObjectId& obj_id, const ObjectId& parent_id, size_t cnt)
{
    if (cnt == 0) return;

    auto state_i = _user_states.find(user);
    ouisync_assert(state_i != _user_states.end());
    if (state_i == _user_states.end()) return;
    auto& state = state_i->second;

    insert_object(state, obj_id, parent_id, cnt);
}

void Index::insert_object(UserState& state, const ObjectId& obj_id, const ObjectId& parent_id, size_t cnt)
{
    if (cnt == 0) return;

    auto i = state.elements.insert({obj_id, {}}).first;
    auto& parents = i->second;
    auto j = parents.insert({parent_id, 0u}).first;
    j->second += cnt;

    if (obj_id == parent_id) {
        state.commit.root_id = obj_id;
    }
}

void Index::remove_object(const UserId& user, const ObjectId& obj_id, const ObjectId& parent_id)
{
    auto state_i = _user_states.find(user);
    assert(state_i != _user_states.end());
    if (state_i == _user_states.end()) return;

    auto& elements = state_i->second.elements;

    auto i = elements.find(obj_id);

    ouisync_assert(i != elements.end());

    auto& parents = i->second;

    auto j = parents.find(parent_id);

    ouisync_assert(j != parents.end());
    ouisync_assert(j->second != 0u);

    if (--j->second == 0) {
        parents.erase(j);
    }

    if (parents.empty()) elements.erase(i);
}

size_t Index::count_object_in_parent(const Elements& elements, const ObjectId& obj_id, const ObjectId& parent_id) const
{
    auto i = elements.find(obj_id);
    if (i == elements.end()) return 0;
    auto j = i->second.find(parent_id);
    if (j == i->second.end()) return 0;
    ouisync_assert(j->second != 0);
    return j->second != 0;
}

//std::ostream& ouisync::operator<<(std::ostream& os, const Index& index)
//{
//    os << index._commit << "\n";
//    for (auto& [obj_id, parents]: index._elements) {
//        os << "  " << obj_id << "\n";
//        for (auto& [parent_id, cnt] : parents) {
//            os << "    " << parent_id << " " << cnt << "\n";
//        }
//    }
//    return os;
//}

bool Index::someone_has(const ObjectId& obj) const
{
    for (auto& [user, state] : _user_states) {
        if (state.elements.find(obj) != state.elements.end()) return true;
    }
    return false;
}

Opt<Commit> Index::commit(const UserId& user)
{
    auto i = _user_states.find(user);
    if (i == _user_states.end()) return boost::none;
    return i->second.commit;
}

std::set<ObjectId> Index::roots() const
{
    std::set<ObjectId> ret;
    for (auto& [user, state] : _user_states) {
        ret.insert(state.commit.root_id);
    }
    return ret;
}

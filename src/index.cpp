#include "index.h"
#include "hash.h"
#include "ouisync_assert.h"
#include "object_store.h"
#include "directory.h"
#include "file_blob.h"
#include "variant.h"
#include "ouisync_assert.h"
#include "iterator_range.h"
#include "ostream/set.h"

#include <iostream>

using namespace ouisync;
using std::string;
using std::move;
using std::set;
using std::cerr;

Index::Index(const UserId& user_id, VersionedObject commit)
{
    _objects[commit.id][commit.id][user_id] = 1;
    _commits[user_id] = move(commit);
}

void Index::set_version_vector(const UserId& user, const VersionVector& vv)
{
    auto ci = _commits.find(user);

    ouisync_assert(ci != _commits.end());
    if (ci == _commits.end()) exit(1);

    if (!vv.happened_after(ci->second.versions)) return;
    if (vv == ci->second.versions) return;
    ci->second.versions = vv;
}

template<class Map, class F>
static
void zip(Map& a, Map& b, F&& modifier)
{
    auto ai = a.begin();
    auto bi = b.begin();

    while (ai != a.end() || bi != b.end()) {
        if (ai == a.end()) {
            auto rni = std::next(bi);
            modifier(bi->first, ai, bi);
            bi = rni;
        }
        else if (bi == b.end()) {
            auto lni = std::next(ai);
            modifier(ai->first, ai, bi);
            ai = lni;
        }
        else if (ai->first == bi->first) {
            auto lni = std::next(ai);
            auto rni = std::next(bi);
            modifier(ai->first, ai, bi);
            ai = lni;
            bi = rni;
        } else {
            if (ai->first < bi->first) {
                auto lni = std::next(ai);
                modifier(ai->first, ai, b.end());
                ai = lni;
            } else {
                auto rni = std::next(bi);
                modifier(bi->first, a.end(), bi);
                bi = rni;
            }
        }
    }
}

struct Index::Item
{
    using Oi = ObjectMap::iterator;
    using Pi = ParentMap::iterator;
    using Ui = UserMap  ::iterator;

    ObjectMap& objects;

    Opt<Oi> oi;
    Opt<Pi> pi;
    Opt<Ui> ui;

    Item(ObjectMap& os)                      : objects(os) {}
    Item(ObjectMap& os, Oi oi)               : objects(os), oi(oi)                 { normalize(); }
    Item(ObjectMap& os, Oi oi, Pi pi)        : objects(os), oi(oi), pi(pi)         { normalize(); }
    Item(ObjectMap& os, Oi oi, Pi pi, Ui ui) : objects(os), oi(oi), pi(pi), ui(ui) { normalize(); }

    // Make so that none of `oi`, `pi`, `ui` are `*.end()`s, but instead they're
    // set to boost::none.
    void normalize() {
        using boost::none;
        if (!oi)                        { oi = none; pi = none; ui = none; return; }
        if (*oi == objects.end())       { oi = none; pi = none; ui = none; return; }
        if (*pi == (*oi)->second.end()) {            pi = none; ui = none; return; }
        if (*ui == (*pi)->second.end()) {                       ui = none; return; }
    }

    operator bool() const { return oi && pi && ui; }

    Index::Count get_count() const {
        ouisync_assert(ui);
        return (*ui)->second;
    }

    void set_count(Index::Count cnt) {
        ouisync_assert(cnt > 0);
        ouisync_assert(ui);
        (*ui)->second = cnt;
    }

    void erase()
    {
        if (ui) {
            (*pi)->second.erase(*ui);
        }
        if (pi && (*pi)->second.empty()) {
            (*oi)->second.erase(*pi);
        }
        if (oi && (*oi)->second.empty()) {
            objects.erase(*oi);
        }
    }
};

template<class F> void Index::compare(const ObjectMap& remote_objects, F&& cmp)
{
    // Const cast below because it would have double the code if the `Item`
    // class had to work with both `::iterator`s and `::const_iterator`s.
    auto& lo = _objects;
    auto& ro = const_cast<ObjectMap&>(remote_objects);

    zip(lo, ro,
        [&] (ObjectId obj, auto obj_li, auto obj_ri) {
            if (obj_li != _objects.end() && obj_ri != remote_objects.end()) {
                zip(parents(obj_li),
                    parents(obj_ri),
                    [&](auto parent_id, auto parent_li, auto parent_ri)
                    {
                        if (parent_li != parents(obj_li).end() && parent_ri != parents(obj_ri).end()) {
                            zip(users(parent_li), users(parent_ri),
                                [&](auto user_id, auto user_li, auto user_ri) {
                                    cmp(obj, parent_id, user_id,
                                        Item(lo, obj_li, parent_li, user_li),
                                        Item(ro, obj_ri, parent_ri, user_ri));
                                });
                        }
                        else if (parent_ri == parents(obj_ri).end()) {
                            for (auto user_li : iterator_range(users(parent_li))) {
                                cmp(obj, parent_id, id(user_li),
                                    Item(lo, obj_li, parent_li, user_li),
                                    Item(ro, obj_ri, parent_ri));
                            }
                        }
                        else if (parent_li == parents(obj_li).end()) {
                            for (auto user_ri : iterator_range(users(parent_ri))) {
                                cmp(obj, parent_id, id(user_ri),
                                    Item(lo, obj_li, parent_li),
                                    Item(ro, obj_ri, parent_ri, user_ri));
                            }
                        }
                        else {
                            ouisync_assert(0);
                        }
                    });
            }
            else if (obj_ri == remote_objects.end()) {
                for (auto parent_li : iterator_range(parents(obj_li))) {
                    for (auto user_li : iterator_range(users(parent_li))) {
                        cmp(obj, id(parent_li), id(user_li),
                            Item(lo, obj_li, parent_li, user_li),
                            Item(ro, obj_ri));
                    }
                }
            }
            else if (obj_li == _objects.end()) {
                for (auto parent_ri : iterator_range(parents(obj_ri))) {
                    for (auto user_ri : iterator_range(users(parent_ri))) {
                        cmp(obj, id(parent_ri), id(user_ri),
                            Item(lo, obj_li),
                            Item(ro, obj_ri, parent_ri, user_ri));
                    }
                }
            }
            else {
                ouisync_assert(0);
            }
        });
}

void Index::merge(const Index& remote_index, ObjectStore& objstore)
{
    auto& remote_commits = remote_index._commits;
    auto& remote_objects = remote_index._objects;

    set<UserId> is_newer;

    for (auto& [remote_user, remote_commit] : remote_commits) {
        if (remote_is_newer(remote_commit, remote_user)) {
            is_newer.insert(remote_user);
        }
    }

    compare(remote_objects,
        [&] (auto& obj_id, auto& parent_id, auto& user_id, Item local, Item remote)
        {
            if (!is_newer.count(user_id)) return;

            if (local && remote) {
                ouisync_assert(remote.get_count() > 0);
                local.set_count(remote.get_count());
            }
            else if (local) {
                auto id = obj_id;

                local.erase();

                if (!someone_has(id)) {
                    objstore.remove(id);
                    _missing_objects.erase(id);
                }
            }
            else if (remote) {
                bool obj_is_new = !someone_has(obj_id);

                auto& um = _objects[obj_id][parent_id];
                auto ui = um.insert({user_id, 0}).first;
                ui->second = remote.get_count();

                ouisync_assert(ui->second > 0);

                if (obj_id == parent_id) {
                    _commits[user_id] = remote_commits.at(user_id);
                }

                if (obj_is_new) _missing_objects.insert(obj_id);
            }
            else {
                ouisync_assert(0);
            }
        });
}

bool Index::remote_is_newer(const VersionedObject& remote_commit, const UserId& user) const
{
    auto li = _commits.find(user);

    if (li == _commits.end()) {
        return remote_commit.versions.version_of(user) > 0;
    } else {
        // Sincle user can't/must not create concurrent versions, so comparing
        // the single version number is sufficcient.
        return remote_commit.versions.version_of(user) > li->second.versions.version_of(user);
    }
}

void Index::insert_object(const UserId& user, const ObjectId& obj_id, const ObjectId& parent_id, size_t cnt)
{
    if (cnt == 0) return;

    auto obj_i    = _objects.insert({obj_id, {}}).first;
    auto parent_i = obj_i->second.insert({parent_id, {}}).first;
    auto user_i   = parent_i->second.insert({user, 0}).first;

    user_i->second += cnt;

    if (obj_id == parent_id) {
        _commits[user].id = obj_id;
    }
}

void Index::remove_object(const UserId& user, const ObjectId& obj_id, const ObjectId& parent_id)
{
    auto obj_i = _objects.find(obj_id);
    ouisync_assert(obj_i != _objects.end());
    if (obj_i == _objects.end()) return;

    auto parent_i = obj_i->second.find(parent_id);
    ouisync_assert(parent_i != obj_i->second.end());
    if (parent_i == obj_i->second.end()) return;

    auto user_i = parent_i->second.find(user);
    ouisync_assert(user_i != parent_i->second.end());
    if (user_i == parent_i->second.end()) return;

    if (--user_i->second == 0) {
        Item(_objects, obj_i, parent_i, user_i).erase();
    }
}

bool Index::someone_has(const ObjectId& obj) const
{
    return _objects.find(obj) != _objects.end();
}

bool Index::object_is_missing(const ObjectId& obj) const
{
    return _missing_objects.find(obj) != _missing_objects.end();
}

bool Index::mark_not_missing(const ObjectId& obj)
{
    return _missing_objects.erase(obj) != 0;
}

Opt<VersionedObject> Index::commit(const UserId& user)
{
    auto i = _commits.find(user);
    if (i == _commits.end()) return boost::none;
    return i->second;
}

std::set<ObjectId> Index::roots() const
{
    std::set<ObjectId> ret;
    for (auto& [user, commit] : _commits) {
        ret.insert(commit.id);
    }
    return ret;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Index& index)
{
    os << "Commits = {\n";
    for (auto& [user, commit] : index._commits) {
        os << "  User:" << user << " Root:" << commit.id << " Versions:" << commit.versions << "\n";
    }
    os << "}\n";
    os << "Objects = {\n";
    for (auto& [obj, parents]: index._objects) {
        for (auto& [parent, users]: parents) {
            for (auto& [user, count]: users) {
                os << "  Object:" << obj << " Parent:" << parent
                    << " User:" << user << " Count:" << count << "\n";
            }
        }
    }
    os << "}\n";

    os << "Missing = " << index._missing_objects << "\n";

    return os;
}

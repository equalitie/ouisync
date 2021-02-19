#pragma once

#include "commit.h"
#include "user_id.h"

#include <map>
#include <set>
#include <string>

namespace ouisync {

class ObjectStore;

class Index {
private:
    template<class K, class V> using Map = std::map<K, V>;
    using ParentId = ObjectId;
    using Count = uint32_t;
    using UserMap = Map<UserId, Count>;
    using ParentMap = Map<ParentId, UserMap>;
    using ObjectMap = Map<ObjectId, ParentMap>;

    const ObjectId& id(ObjectMap::const_iterator i) { return i->first; }
    const ParentId& id(ParentMap::const_iterator i) { return i->first; }
    const UserId&   id(UserMap  ::const_iterator i) { return i->first; }

          ParentMap& parents(ObjectMap::iterator       i) { return i->second; }
    const ParentMap& parents(ObjectMap::const_iterator i) { return i->second; }

          UserMap& users(ParentMap::iterator       i) { return i->second; }
    const UserMap& users(ParentMap::const_iterator i) { return i->second; }

    const Count& count(UserMap::const_iterator i) { return i->second; }
          Count& count(UserMap::iterator i) { return i->second; }

    struct Item;

public:
    Index() {}
    Index(const UserId&, Commit);

    void set_commit(const UserId&, const Commit&);
    void set_version_vector(const UserId&, const VersionVector&);

    void insert_object(const UserId&, const ObjectId& id, const ParentId& parent_id, size_t cnt = 1);
    void remove_object(const UserId&, const ObjectId& id, const ParentId& parent_id);

    void merge(const Index&, ObjectStore&);

    Opt<Commit> commit(const UserId&);

    friend std::ostream& operator<<(std::ostream&, const Index&);

    const std::set<ObjectId>& missing_objects() const { return _missing_objects; }

    bool someone_has(const ObjectId&) const;

    std::set<ObjectId> roots() const;

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _objects & _commits & _missing_objects;
    }

    bool remote_is_newer(const Commit& remote_commit, const UserId&) const;

    friend std::ostream& operator<<(std::ostream&, const Index&);

private:
    template<class F> void compare(const ObjectMap&, F&&);

private:
    ObjectMap _objects;
    Map<UserId, Commit> _commits;
    std::set<ObjectId> _missing_objects;
};

} // namespace

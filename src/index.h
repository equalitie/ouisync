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
    using ParentId = ObjectId;
    using Elements = std::map<ObjectId, std::map<ParentId, uint32_t>>;
    //                                              |         |
    //                                   Parent <---+         |
    //                                                        |
    //  How many times is the object listed in the parent <---+

public:
    Index() {}
    Index(const UserId&);

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
        ar & _user_states & _missing_objects;
    }

private:
    struct UserState {
        Commit commit;
        Elements elements;

        template<class Archive>
        void serialize(Archive& ar, unsigned) {
            ar & commit & elements;
        }

        UserState();
    };

    template<class F> static void remove_count(Elements&, F&& f);

    template<class F> static void for_each(const Elements&, F&& f);
    static void count_objects_in_parent(const Elements&, const ObjectId&, const ParentId&);

    static
    void insert_object(UserState&, const ObjectId& id, const ParentId& parent_id, size_t cnt);

    size_t count_object_in_parent(const Elements&, const ObjectId& obj_id, const ObjectId& parent_id) const;

private:
    std::map<UserId, UserState> _user_states;
    std::set<ObjectId> _missing_objects;
};

} // namespace

#pragma once

#include "user_id.h"
#include <map>

namespace ouisync {

class VersionVector {
public:
    using Version = uint64_t;

private:
    using Map = std::map<UserId, Version>;

public:

    void increment(const UserId& id)
    {
        auto iter = _vv.insert(std::make_pair(id, 0U)).first;
        ++value_of(iter);
    }

    VersionVector merge(const VersionVector& that_vv) const
    {
        VersionVector result;

        const Map& that = that_vv._vv;
        auto this_i = _vv.begin();
        auto that_i = that.begin();

        while (this_i != _vv.end() || that_i != that.end()) {
            if (this_i == _vv.end()) {
                result._vv[id_of(that_i)] = value_of(that_i);
                ++that_i;
                continue;
            }
            else if (that_i == that.end()) {
                result._vv[id_of(this_i)] = value_of(this_i);
                ++this_i;
                continue;
            }

            if (id_of(this_i) < id_of(that_i)) {
                result._vv[id_of(this_i)] = value_of(this_i);
                ++this_i;
            }
            else if (id_of(that_i) < id_of(this_i)) {
                result._vv[id_of(that_i)] = value_of(that_i);
                ++that_i;
            }
            else {
                result._vv[id_of(this_i)] = std::max(value_of(this_i), value_of(that_i));
                ++this_i;
                ++that_i;
            }
        }

        return result;
    }

    // This one does a standard std::map comparison of keys and values. It does NOT
    // express causal relationship. It is mostly useful for storing VersionVector
    // in collections such as std::set which require this operator.
    bool operator<(const VersionVector& other) const {
        return _vv < other._vv;
    }

    bool happened_before(const VersionVector& that_vv) const {
        const Map& that = that_vv._vv;

        auto this_i = _vv.begin();
        auto that_i = that.begin();

        bool that_has_bigger = false;

        while (true) {
            if (this_i == _vv.end()) {
                if (that_i != that.end()) that_has_bigger = true;
                return that_has_bigger;
            }
            else if (that_i == that.end()) {
                // We know (this_i != _vv.end()) because otherwise we'd be in the
                // above branch. Thus we know 'this' has a bigger value and therefore
                // couldn't have happened before 'that'.
                return false;
            }

            if (id_of(this_i) < id_of(that_i)) {
                // This has a bigger value, so coudln't have happened before.
                return false;
            }
            else if (id_of(that_i) < id_of(this_i)) {
                that_has_bigger = true;
                ++that_i;
            }
            else {
                if (value_of(this_i) != value_of(that_i)) {
                    if (value_of(this_i) < value_of(that_i)) {
                        that_has_bigger = true;
                    } else {
                        // This has a bigger value, so coudln't have happened
                        // before.
                        return false;
                    }
                }
                ++this_i;
                ++that_i;
            }
        }

        return true;
    }

    bool happened_after(const VersionVector& that_vv) const {
        return that_vv.happened_before(*this);
    }

    bool operator==(const VersionVector& that_vv) const {
        return _vv == that_vv._vv;
    }

    bool operator!=(const VersionVector& that_vv) const {
        return _vv != that_vv._vv;
    }

    template<class Archive>
    void serialize(Archive& ar, unsigned version) {
        ar & _vv;
    }

    size_t size() const { return _vv.size(); }
    auto begin() const { return _vv.begin(); }
    auto end()   const { return _vv.end();   }

    template<class Hash>
    friend void update_hash(const VersionVector& vv, Hash& hash)
    {
        hash.update(uint32_t(vv.size()));
        for (auto& [k, v] : vv._vv) {
            hash.update(k);
            hash.update(v);
        }
    }

    Version version_of(const UserId& uid) const {
        auto i = _vv.find(uid);
        if (i == _vv.end()) return 0u;
        assert(i->second != 0u);
        return i->second;
    }

    void set_version(const UserId& uid, Version v) {
        if (v == 0) {
            auto i = _vv.find(uid);
            if (i == _vv.end()) return;
            _vv.erase(i);
        } else {
            _vv[uid] = v;
        }
    }

private:
    static
    const UserId& id_of(const Map::const_iterator& i)
    {
        return i->first;
    }

    static
    Version& value_of(Map::iterator& i)
    {
        return i->second;
    }

    static
    const Version& value_of(const Map::const_iterator& i)
    {
        return i->second;
    }

private:
    Map _vv;
};

} // namespace

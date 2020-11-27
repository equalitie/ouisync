#pragma once

#include "user_id.h"
#include <map>

namespace ouisync {

class VersionVector {
private:
    using Version = uint64_t;
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

    bool operator<=(const VersionVector& that_vv) const {
        const Map& that = that_vv._vv;

        auto this_i = _vv.begin();
        auto that_i = that.begin();

        while (true) {
            if (this_i == _vv.end()) {
                return true;
            }
            else if (that_i == that.end()) {
                if (value_of(this_i) > 0) return false;
                ++this_i;
                continue;
            }

            if (id_of(this_i) < id_of(that_i)) {
                if (value_of(this_i) > 0) return false;
                ++this_i;
            }
            else if (id_of(that_i) < id_of(this_i)) {
                ++that_i;
            }
            else {
                if (value_of(this_i) != value_of(that_i)) {
                    return value_of(this_i) < value_of(that_i);
                }
                ++this_i;
                ++that_i;
            }
        }

        return true;
    }

    template<class Archive>
    void serialize(Archive& ar, unsigned version) {
        ar & _vv;
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

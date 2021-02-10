#include "directory.h"
#include "version_vector.h"

#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;

ObjectId Directory::calculate_id() const
{
    Sha256 hash;
    hash.update(static_cast<std::underlying_type_t<ObjectTag>>(tag));
    hash.update(uint32_t(size()));
    for (auto& [filename,user_map] : *this) {
        hash.update(filename);

        hash.update(uint32_t(user_map.size()));

        for (auto& [user, vobj] : user_map) {
            hash.update(user);
            hash.update(vobj.object_id);
            hash.update(vobj.version_vector);
        }
    }

    return hash.close();
}

VersionVector Directory::calculate_version_vector_union() const
{
    VersionVector result;

    for (auto& [filename, user_map] : _name_map) {
        for (auto& [username, vobj] : user_map) {
            result = result.merge(vobj.version_vector);
        }
    }

    return result;
}

struct Padding {
    unsigned level = 0;
    Padding(unsigned level) : level(level) {}

    friend std::ostream& operator<<(std::ostream& os, Padding pad) {
        for (unsigned i = 0; i < pad.level; ++i) { os << " "; }
        return os;
    }
};

void Directory::print(std::ostream& os, unsigned level) const
{
    os << Padding(level*4) << "Directory id:" << calculate_id() << "\n";
    for (auto& [filename, user_map] : _name_map) {
        os << Padding(level*4) << "  filename:" << filename << "\n";
        for (auto& [user, vobj]: user_map) {
            os << Padding(level*4) << "    user:" << user << "\n";
            os << Padding(level*4) << "    obj:"  << vobj.object_id << "\n";
        }
    }
}

std::ostream& ouisync::operator<<(std::ostream& os, const Directory& tree) {
    tree.print(os, 0);
    return os;
}

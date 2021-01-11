#include "object_id.h"
#include "hex.h"
#include <ostream>
#include <map>

using namespace ouisync;

using NameMap = std::map<ObjectId, std::list<std::string>>;

/* static */
NameMap g_debug_name_map;

ObjectId::RiaaNameMap::~RiaaNameMap() {
    map_iter->second.erase(list_iter);
    if (map_iter->second.empty()) {
        g_debug_name_map.erase(map_iter);
    }
}

ObjectId::RiaaNameMap ObjectId::debug_name(const ObjectId& id, std::string name)
{
    auto i = g_debug_name_map.insert({id, {}}).first;
    i->second.push_back(std::move(name));
    return RiaaNameMap{i, std::prev(i->second.end())};
}

ObjectId::Hex ObjectId::hex() const
{
    auto hex = to_hex<char>(*this);
    return hex;
}

ObjectId::ShortHex ObjectId::short_hex() const
{
    auto hex = to_hex<char>(*this);

    ShortHex ret;

    for (auto i = 0u; i < ShortHex::size; i++)
        ret[i] = hex[i];

    return ret;
}

std::ostream& ouisync::operator<<(std::ostream& os, const ObjectId::Hex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const ObjectId::ShortHex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const ObjectId& id)
{
    auto i = g_debug_name_map.find(id);
    if (i != g_debug_name_map.end()) {
        assert(!i->second.empty());
        if (!i->second.empty()) {
            return os << i->second.back();
        }
    }
    return os << id.short_hex();
}

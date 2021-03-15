#include "block_id.h"
#include "hex.h"
#include <ostream>
#include <map>

using namespace ouisync;

using NameMap = std::map<BlockId, std::list<std::string>>;

/* static */
NameMap g_debug_name_map;

/* static */
BlockId BlockId::null_id() {
    static Opt<Parent> id;
    if (!id) {
        id = Parent{};
        id->fill(0);
    }
    return *id;
}

BlockId::Hex BlockId::hex() const
{
    auto hex = to_hex<char>(*this);
    return hex;
}

BlockId::ShortHex BlockId::short_hex() const
{
    auto hex = to_hex<char>(*this);

    ShortHex ret;

    for (auto i = 0u; i < ShortHex::size; i++)
        ret[i] = hex[i];

    return ret;
}

std::ostream& ouisync::operator<<(std::ostream& os, const BlockId::Hex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const BlockId::ShortHex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const BlockId& id)
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

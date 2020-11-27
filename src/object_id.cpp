#include "object_id.h"
#include "hex.h"
#include <ostream>

using namespace ouisync;

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
    return os << id.short_hex();
}

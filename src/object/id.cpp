#include "id.h"
#include "hex.h"
#include <ostream>

using namespace ouisync;
using namespace ouisync::object;

Id::Hex Id::hex() const
{
    auto hex = to_hex<char>(*this);
    return hex;
}

Id::ShortHex Id::short_hex() const
{
    auto hex = to_hex<char>(*this);

    ShortHex ret;

    for (auto i = 0u; i < ShortHex::size; i++)
        ret[i] = hex[i];

    return ret;
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Id::Hex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Id::ShortHex& h)
{
    for (auto c : h) { os << c; }
    return os;
}

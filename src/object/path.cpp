#include "path.h"
#include "../hex.h"

using namespace ouisync;
using namespace ouisync::object;

fs::path path::from_id(const Id& id)
{
    auto hex = to_hex<char>(id);
    string_view hex_sv{hex.data(), hex.size()};

    static const size_t prefix_size = 3;

    auto prefix = hex_sv.substr(0, prefix_size);
    auto rest   = hex_sv.substr(prefix_size, hex_sv.size() - prefix_size);

    return fs::path(prefix.begin(), prefix.end()) / fs::path(rest.begin(), rest.end());
}

Opt<Id> path::to_id(const fs::path& ps)
{
    std::array<char, std::tuple_size<Id>::value * 2> hex;

    auto i = hex.begin();

    for (auto& p : ps) {
        for (auto c : p.native()) {
            if (i == hex.end()) return boost::none;
            *i++ = c;
        }
    }

    if (i != hex.end()) return boost::none;

    return from_hex<uint8_t>(hex);
}

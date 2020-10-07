#include "object.h"
#include "tagged.h"
#include "../hex.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/serialization/string.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using namespace ouisync::objects;

/* static */
fs::path Object::path_from_digest(const Sha256::Digest& digest) {
    auto hex_digest = to_hex<char>(digest);
    string_view hex_digest_str{hex_digest.data(), hex_digest.size()};
    static const size_t prefix_size = 3;
    auto prefix = hex_digest_str.substr(0, prefix_size);
    auto rest   = hex_digest_str.substr(prefix_size, hex_digest_str.size() - prefix_size);
    return fs::path(prefix.begin(), prefix.end()) / fs::path(rest.begin(), rest.end());
}

Sha256::Digest Object::calculate_digest() const {
    return ouisync::apply(_variant, [] (const auto& v) { return v.calculate_digest(); });
}

Object Object::load(const fs::path& root, const Sha256::Digest& digest)
{
    fs::ifstream ifs(root / path_from_digest(digest), fs::ifstream::binary);
    if (!ifs.is_open())
        throw std::runtime_error("Failed to open object");
    boost::archive::text_iarchive ia(ifs);
    Object result;
    tagged::Load loader{result._variant};
    ia >> loader;
    return result;
}

Id Object::store(const fs::path& root) const
{
    return ouisync::apply(_variant, [&root] (const auto& v) { return v.store(root); });
}

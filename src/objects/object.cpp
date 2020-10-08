#include "object.h"
#include "tagged.h"
#include "../hex.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/serialization/string.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using namespace ouisync::objects;

Sha256::Digest Object::calculate_digest() const {
    return ouisync::apply(_variant, [] (const auto& v) { return v.calculate_digest(); });
}

Id Object::store(const fs::path& root) const
{
    return ouisync::apply(_variant, [&root] (const auto& v) { return v.store(root); });
}

#pragma once

#include "block.h"
#include "tree.h"
#include "id.h"
#include "../variant.h"
#include "../namespaces.h"

#include <boost/filesystem/path.hpp>

namespace ouisync::objects {

class Object final {
public:
    using Variant = boost::variant<Block, Tree>;

public:
    Object() = default;
    Object(Object&&) = default;
    Object(const Object&) = delete;

    static
    Object load(const fs::path& root, const Sha256::Digest&);

    Block* as_block() { return boost::get<Block>(&_variant); }
    Tree * as_tree () { return boost::get<Tree >(&_variant); }
    
    const Block* as_block() const { return boost::get<Block>(&_variant); }
    const Tree * as_tree () const { return boost::get<Tree >(&_variant); }

    Sha256::Digest calculate_digest() const;

    Id store(const fs::path& root) const;

    static
    fs::path path_from_digest(const Sha256::Digest& digest);

private:
    Variant _variant;
};


} // namespace

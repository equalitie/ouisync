#pragma once

#include "block.h"
#include "tree.h"
#include "id.h"
#include "../variant.h"
#include "../namespaces.h"

#include <boost/filesystem/path.hpp>

namespace ouisync::object {

class Any final {
public:
    using Variant = boost::variant<Block, Tree>;

public:
    Any() = default;
    Any(Any&&) = default;
    Any(const Any&) = delete;

    Block* as_block() { return boost::get<Block>(&_variant); }
    Tree * as_tree () { return boost::get<Tree >(&_variant); }
    
    const Block* as_block() const { return boost::get<Block>(&_variant); }
    const Tree * as_tree () const { return boost::get<Tree >(&_variant); }

    Id calculate_id() const;

    Id store(const fs::path& root) const;

private:
    Variant _variant;
};


} // namespace

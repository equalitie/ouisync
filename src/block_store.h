#pragma once

#include "object_id.h"
#include "block.h"

#include <boost/optional.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync {

class BlockStore {
public:
    BlockStore(const fs::path& blockdir);

    static Block load(const fs::path&);
    Block load(const BlockId&) const;
    Opt<Block> maybe_load(const BlockId&) const;

    BlockId store(const char*, size_t);
    BlockId store(const Block&);
    void store(const BlockId&, const Block&);
    void store(const BlockId&, const char*, size_t);

    static BlockId calculate_block_id(const Block&);
    static BlockId calculate_block_id(const char*, size_t);

    void remove(const BlockId&);

private:
    fs::path id_to_path(const BlockId&) const;

private:
    fs::path _blockdir;
};

} // namespace

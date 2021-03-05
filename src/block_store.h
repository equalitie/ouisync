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
    Block load(const ObjectId&) const;
    Opt<Block> maybe_load(const ObjectId&) const;

    ObjectId store(const char*, size_t);
    ObjectId store(const Block&);
    void store(const ObjectId&, const Block&);
    void store(const ObjectId&, const char*, size_t);

    static ObjectId calculate_block_id(const Block&);
    static ObjectId calculate_block_id(const char*, size_t);

    void remove(const ObjectId&);

private:
    fs::path id_to_path(const ObjectId&) const;

private:
    fs::path _blockdir;
};

} // namespace

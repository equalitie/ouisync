#pragma once

#include "object_id.h"

#include <vector>
#include <boost/optional.hpp>

namespace ouisync {

class ObjectStore;

class BlockStore {
public:
    using Block = std::vector<char>;

public:
    BlockStore(ObjectStore&);

    Block load(const ObjectId&);
    Opt<Block> maybe_load(const ObjectId&);

    ObjectId store(const Block&);
    void store(const ObjectId&, const Block&);

    static ObjectId calculate_block_id(const Block&);

private:
    fs::path id_to_path(const ObjectId&);

private:
    // XXX: Temporarily using ObjectStore
    ObjectStore& _objstore;
};

} // namespace

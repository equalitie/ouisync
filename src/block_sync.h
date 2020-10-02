#pragma once

#include <boost/variant.hpp>
#include "version_vector.h"
#include "block_id.h"
#include "hash.h"

#include <mutex>

namespace ouisync {

class BlockSync {
    using Digest = Sha256::Digest;

public:
    struct ActionModifyBlock {
        BlockId id;
        Digest digest;
    };
    struct ActionRemoveBlock {
        BlockId id;
    };

    using Action = boost::variant<
        ActionModifyBlock,
        ActionRemoveBlock>;

    void add_action(Action&&);

private:
    void insert_block(const BlockId&, const Digest&);
    void remove_block(const BlockId&);

private:
    VersionVector::Uuid _uuid;
    VersionVector _version_vector;
    std::map<BlockId, Digest> _blocks;
    std::mutex _mutex;
};

}

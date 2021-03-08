#pragma once

#include "block.h"
#include "object_id.h"

#include <boost/functional/hash.hpp> // hash a pair
#include <unordered_set>
#include <unordered_map>

namespace ouisync {

class BlockStore;
class UserId;
class Index;

class Transaction {
private:
    template<class... Args> using Set = std::unordered_set<Args...>;
    template<class... Args> using Map = std::unordered_map<Args...>;

    using Blocks = Map<ObjectId, Block>;

    using Edge  = std::pair<ObjectId, ObjectId>;
    using Edges = Set<Edge, boost::hash<Edge>>;

public:
    void insert_block(const ObjectId& id, Block block) {
        _blocks.emplace(id, std::move(block));
    }

    void insert_edge(const ObjectId& from, const ObjectId& to) {
        _edges.emplace(from, to);
    }

    const Blocks& blocks() const {
        return _blocks;
    }

    const Edges& edges() const {
        return _edges;
    }

    void commit(const UserId&, BlockStore&, Index&);

private:
    Blocks _blocks;
    Edges  _edges;
};

} // namespace

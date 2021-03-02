#pragma once

#include "object_id.h"
#include "object_store.h"

#include <vector>
#include <boost/optional.hpp>

#include <boost/serialization/array_wrapper.hpp>
#include <boost/serialization/split_member.hpp>

namespace ouisync {

class BlockStore {
public:
    struct Block : std::vector<char>
    {
        using std::vector<char>::vector;

        template<class Archive>
        void save(Archive& ar, const unsigned int version) const {
            ar & uint32_t(size());
            ar & boost::serialization::make_array(data(), size());
        }

        template<class Archive>
        void load(Archive& ar, const unsigned int version) {
            uint32_t size;
            ar & size;
            resize(size);
            ar & boost::serialization::make_array(data(), size);
        }

        BOOST_SERIALIZATION_SPLIT_MEMBER()
    };

public:
    BlockStore(const fs::path& blockdir);

    Block load(const fs::path&) const;
    Block load(const ObjectId&) const;
    Opt<Block> maybe_load(const ObjectId&) const;

    ObjectId store(const char*, size_t);
    ObjectId store(const Block&);
    void store(const ObjectId&, const Block&);

    static ObjectId calculate_block_id(const Block&);
    static ObjectId calculate_block_id(const char*, size_t);

    void remove(const ObjectId&);

private:
    fs::path id_to_path(const ObjectId&) const;

private:
    // XXX: Temporarily using ObjectStore
    ObjectStore _objstore;
};

} // namespace

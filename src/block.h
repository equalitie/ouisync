#pragma once

#include <vector>
#include <boost/serialization/split_member.hpp>
#include <boost/serialization/array_wrapper.hpp>

namespace ouisync {

class Block : public std::vector<char>
{
public:
    Block() = default;
    Block(const Block&) = default;
    Block(Block&&) = default;

    Block& operator=(const Block&) = default;
    Block& operator=(Block&&) = default;

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

} // ouisync namespace

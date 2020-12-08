#pragma once

#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>

namespace ouisync {

// Using subclassing instead of typedefs to be able to use forward declaration.

class InputArchive : public boost::archive::binary_iarchive
{
    public:
    using boost::archive::binary_iarchive::binary_iarchive;
};

class OutputArchive : public boost::archive::binary_oarchive
{
    public:
    using boost::archive::binary_oarchive::binary_oarchive;
};

} // namespace

#pragma once

#include <boost/uuid/uuid.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <boost/uuid/uuid_serialize.hpp>

#include "namespaces.h"

namespace ouisync {

class UserId {
public:
    static UserId load_or_create(const fs::path& file_path);

    UserId() = default;
    UserId(const UserId&) = default;
    UserId(UserId&&) = default;

    UserId& operator=(const UserId&) = default;
    UserId& operator=(UserId&&) = default;

    std::string to_string() const;

    bool operator<(const UserId& other) const {
        return _uuid < other._uuid;
    }

    template<class Archive>
    void serialize(Archive& ar, unsigned version) {
        ar & _uuid;
    }

private:
    UserId(const boost::uuids::uuid&);

private:
    boost::uuids::uuid _uuid;
};

} // namespace

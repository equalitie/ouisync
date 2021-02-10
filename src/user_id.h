#pragma once

#include <boost/uuid/uuid.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <boost/uuid/uuid_serialize.hpp>

#include "shortcuts.h"

namespace ouisync {

class UserId {
public:
    static UserId load_or_generate_random(const fs::path& file_path);

    static UserId generate_random();

    UserId() = default;
    UserId(const UserId&) = default;
    UserId(UserId&&) = default;

    UserId& operator=(const UserId&) = default;
    UserId& operator=(UserId&&) = default;

    std::string to_string() const;

    static Opt<UserId> from_string(string_view);

    bool operator<(const UserId& other) const {
        return _uuid < other._uuid;
    }

    template<class Archive>
    void serialize(Archive& ar, unsigned version) {
        ar & _uuid;
    }

    friend std::ostream& operator<<(std::ostream& os, const UserId& id) {
        return os << id._uuid;
    }

    bool operator==(const UserId& other) const {
        return _uuid == other._uuid;
    }

    template<class Hash>
    friend void update_hash(const UserId& uid, Hash& hash)
    {
        hash.update(uid._uuid.begin(), boost::uuids::uuid::static_size());
    }

private:
    UserId(const boost::uuids::uuid&);

private:
    boost::uuids::uuid _uuid;
};

} // namespace

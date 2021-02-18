#include "user_id.h"
#include "hex.h"
#include "ostream/array.h"

#include <boost/filesystem.hpp>
#include <boost/uuid/uuid_generators.hpp>
#include <boost/lexical_cast.hpp>
#include <boost/optional.hpp>
#include <boost/utility/string_view.hpp>

using namespace ouisync;

/* static */
UserId UserId::load_or_generate_random(const fs::path& file_path)
{
    fs::fstream f(file_path, f.binary | f.in);

    if (!f.is_open()) {
        if (fs::exists(file_path)) {
            throw std::runtime_error("Failed to open user id file");
        }

        f.open(file_path, f.binary | f.out | f.trunc);

        if (!f.is_open()) {
            throw std::runtime_error("Failed to open file for storing user id");
        }

        auto uuid = boost::uuids::random_generator()();
        f << uuid << "\n";

        return uuid;
    }

    boost::uuids::uuid uuid;
    f >> uuid;
    return uuid;
}

/* static */
UserId UserId::generate_random()
{
    return boost::uuids::random_generator()();
}

UserId::UserId(const boost::uuids::uuid& uuid) :
    _uuid(uuid)
{}

std::string UserId::to_string() const
{
    return boost::uuids::to_string(_uuid);
}

/* static */
Opt<UserId> UserId::from_string(string_view s)
try {
    return UserId(boost::lexical_cast<boost::uuids::uuid>(s));
} catch (...) {
    return boost::none;
}

std::ostream& ouisync::operator<<(std::ostream& os, const UserId& id) {
    auto p = id._uuid.data;
    std::array<uint8_t, 3> a = { p[0], p[1], p[2] };
    return os << to_hex<char>(a);
}

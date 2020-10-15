#include "user_id.h"

#include <boost/filesystem.hpp>
#include <boost/uuid/uuid_generators.hpp>

using namespace ouisync;

/* static */
UserId UserId::load_or_create(const fs::path& file_path)
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

UserId::UserId(const boost::uuids::uuid& uuid) :
    _uuid(uuid)
{}

std::string UserId::to_string() const
{
    return boost::uuids::to_string(_uuid);
}

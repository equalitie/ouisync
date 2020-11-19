#include "snapshot.h"
#include "hash.h"
#include "hex.h"
#include "variant.h"
#include "object/refcount.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/serialization/vector.hpp>

#include <iostream>

using namespace ouisync;

static object::Id calculate_id(const std::set<Commit>& commits)
{
    Sha256 hash;
    hash.update(uint32_t(commits.size()));
    for (auto& commit : commits) hash.update(commit.root_object_id);
    return hash.close();
}

Snapshot::Snapshot(const Id& id, fs::path path, fs::path objdir, Commits commits) :
    _is_valid(true),
    _id(id),
    _path(std::move(path)),
    _objdir(std::move(objdir)),
    _commits(std::move(commits))
{}

Snapshot::Snapshot(Snapshot&& other) :
    _is_valid(other._is_valid),
    _id(other._id),
    _path(std::move(other._path)),
    _objdir(std::move(other._objdir)),
    _commits(std::move(other._commits))
{
    other._is_valid = false;
}

Snapshot& Snapshot::operator=(Snapshot&& other)
{
    destroy();

    std::swap(_is_valid, other._is_valid);
    _id = other._id;
    _path = std::move(other._path);
    _objdir = std::move(other._objdir);
    _commits = std::move(other._commits);

    return *this;
}

Snapshot::Object Snapshot::load_object(const Id& id)
{
    return object::io::load<Blob, Tree>(_objdir, id);
}

/* static */
void Snapshot::store_commits(const fs::path& path, const Commits& commits)
{
    fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);
    assert(ofs.is_open());

    if (!ofs.is_open())
        throw std::runtime_error("Failed to store object");

    boost::archive::binary_oarchive oa(ofs);
    oa << commits;
}

/* static */
Snapshot::Commits Snapshot::load_commits(const fs::path& path)
{
    fs::ifstream ifs(path, ifs.binary);

    if (!ifs.is_open())
        throw std::runtime_error("Failed to open object");

    boost::archive::binary_iarchive ia(ifs);
    Commits commits;
    ia >> commits;
    return commits;
}

/* static */
Snapshot Snapshot::create(const fs::path& snapshotdir, fs::path objdir, Commits commits)
{
    auto id = calculate_id(commits);

    auto id_hex = to_hex<char>(id);
    auto path = snapshotdir / fs::path(id_hex.begin(), id_hex.end());

    // XXX: Handle failures

    for (auto& commit : commits) {
        object::refcount::increment(objdir, commit.root_object_id);
    }

    store_commits(path, commits);
    object::refcount::increment(path);

    return Snapshot(id, std::move(path), std::move(objdir), std::move(commits));
}

void Snapshot::destroy() noexcept
{
    if (!_is_valid) return;

    _is_valid = false;

    // XXX: Handle failures

    try {
        for (auto& commit : _commits) {
            object::refcount::decrement(_objdir, commit.root_object_id);
        }

        object::refcount::decrement(_path);
    }
    catch (const std::exception& e) {
        std::cerr << "Snapshot::destroy() attempted to throw an exception\n";
        exit(1);
    }
}

Snapshot::~Snapshot()
{
    destroy();
}

#include "snapshot.h"
#include "hash.h"
#include "hex.h"
#include "variant.h"
#include "refcount.h"
#include "archive.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/serialization/vector.hpp>

#include <iostream>

using namespace ouisync;

static ObjectId calculate_id(const Commit& commit)
{
    Sha256 hash;
    hash.update("Snapshot");
    hash.update(commit.root_id);
    return hash.close();
}

Snapshot::Snapshot(const ObjectId& id, fs::path path, fs::path objdir, Commit commit) :
    _is_valid(true),
    _id(id),
    _path(std::move(path)),
    _objdir(std::move(objdir)),
    _commit(std::move(commit))
{}

Snapshot::Snapshot(Snapshot&& other) :
    _is_valid(other._is_valid),
    _id(other._id),
    _path(std::move(other._path)),
    _objdir(std::move(other._objdir)),
    _commit(std::move(other._commit))
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
    _commit = std::move(other._commit);

    return *this;
}

/* static */
void Snapshot::store_commit(const fs::path& path, const Commit& commit)
{
    fs::ofstream ofs(path, ofs.out | ofs.binary | ofs.trunc);
    assert(ofs.is_open());

    if (!ofs.is_open())
        throw std::runtime_error("Failed to store object");

    OutputArchive oa(ofs);
    oa << commit;
}

/* static */
Commit Snapshot::load_commit(const fs::path& path)
{
    fs::ifstream ifs(path, ifs.binary);

    if (!ifs.is_open())
        throw std::runtime_error("Failed to open object");

    InputArchive ia(ifs);
    Commit commit;
    ia >> commit;
    return commit;
}

/* static */
Snapshot Snapshot::create(const fs::path& snapshotdir, fs::path objdir, Commit commit)
{
    auto id = calculate_id(commit);

    auto id_hex = to_hex<char>(id);
    auto path = snapshotdir / fs::path(id_hex.begin(), id_hex.end());

    // XXX: Handle failures

    refcount::increment(objdir, commit.root_id);

    store_commit(path, commit);
    refcount::increment(path);

    return Snapshot(id, std::move(path), std::move(objdir), std::move(commit));
}

void Snapshot::destroy() noexcept
{
    if (!_is_valid) return;

    _is_valid = false;

    // XXX: Handle failures

    try {
        refcount::decrement(_objdir, _commit.root_id);
        refcount::decrement(_path);
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

SnapshotGroup::Id SnapshotGroup::calculate_id() const
{
    Sha256 hash;
    hash.update("SnapshotGroup");
    hash.update(uint32_t(size()));
    for (auto& [user_id, snapshot] : *this) {
        hash.update(user_id.to_string());
        hash.update(snapshot.id());
    }
    return hash.close();
}

std::ostream& ouisync::operator<<(std::ostream& os, const Snapshot& s)
{
    return os << "id:" << s._id << " root:" << s._commit.root_id;
}

std::ostream& ouisync::operator<<(std::ostream& os, const SnapshotGroup& g)
{
    os << "SnapshotGroup{id:" << g.id() << " [";
    bool is_first = true;
    for (auto& [user_id, snapshot] : g) {
        if (!is_first) { os << ", "; }
        is_first = false;
        os << "(" << user_id << ", " << snapshot.id() << ", " << snapshot.commit().root_id << ")";
    }
    return os << "]}";
}

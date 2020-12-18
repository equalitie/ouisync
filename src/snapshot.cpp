#include "snapshot.h"
#include "hash.h"
#include "hex.h"
#include "variant.h"
#include "refcount.h"
#include "archive.h"
#include "random.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"
#include "ouisync_assert.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/serialization/vector.hpp>

#include <iostream>

using namespace ouisync;

ObjectId Snapshot::calculate_id() const
{
    Sha256 hash;
    hash.update("Snapshot");
    hash.update(_commit.root_id);
    hash.update(uint32_t(_captured_objs.size()));
    for (auto& id : _captured_objs) {
        hash.update(id);
    }
    return hash.close();
}

Snapshot::Snapshot(fs::path path, fs::path objdir, Commit commit) :
    _path(std::move(path)),
    _objdir(std::move(objdir)),
    _commit(std::move(commit))
{}

Snapshot::Snapshot(Snapshot&& other) :
    _path(std::move(other._path)),
    _objdir(std::move(other._objdir)),
    _commit(std::move(other._commit)),
    _captured_objs(std::move(other._captured_objs))
{}

Snapshot& Snapshot::operator=(Snapshot&& other)
{
    destroy();

    _path = std::move(other._path);
    _objdir = std::move(other._objdir);
    _commit = std::move(other._commit);
    _captured_objs = std::move(other._captured_objs);

    return *this;
}

void Snapshot::capture_object(const ObjectId& obj)
{
    auto [_, inserted] = _captured_objs.insert(obj);
    if (!inserted) return;
    refcount::increment_recursive(_objdir, obj);
}

void Snapshot::store()
{
    archive::store(_path, _captured_objs);
}

static auto _random_file_name(const fs::path& dir)
{
    std::array<unsigned char, 16> rnd;
    random::generate_non_blocking(rnd.data(), rnd.size());
    auto hex = to_hex<char>(rnd);
    return dir / fs::path(hex.begin(), hex.end());
}

/* static */
Snapshot Snapshot::create(const fs::path& snapshotdir, fs::path objdir, Commit commit)
{
    auto path = _random_file_name(snapshotdir);
    Snapshot s(std::move(path), std::move(objdir), std::move(commit));
    s.store();
    return s;
}

void Snapshot::destroy() noexcept
{
    try {
        for (auto id : _captured_objs) {
            refcount::flat_remove(_objdir, id);
        }
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
        hash.update(snapshot.calculate_id());
    }
    return hash.close();
}

std::ostream& ouisync::operator<<(std::ostream& os, const Snapshot& s)
{
    return os << "id:" << s.calculate_id() << " root:" << s._commit.root_id;
}

std::ostream& ouisync::operator<<(std::ostream& os, const SnapshotGroup& g)
{
    os << "SnapshotGroup{id:" << g.id() << " [";
    bool is_first = true;
    for (auto& [user_id, snapshot] : g) {
        if (!is_first) { os << ", "; }
        is_first = false;
        os << "(" << user_id << ", " << snapshot << ")";
    }
    return os << "]}";
}

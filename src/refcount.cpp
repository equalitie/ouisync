#include "refcount.h"
#include "object/tree.h"
#include "object/blob.h"
#include "variant.h"
#include "ouisync_assert.h"
#include "object_store.h"
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>
#include <boost/endian/conversion.hpp>

#include <iostream>
#include <sstream>

#include <boost/serialization/vector.hpp>

using namespace ouisync;

using object::Tree;
using object::Blob;

static
fs::path object_path(const fs::path& objdir, const ObjectId& id) noexcept {
    return objdir / object::path::from_id(id);
}

static
fs::path refcount_path(fs::path path) noexcept {
    path.concat(".rc");
    return path;
}

static
Rc::Number read_number(std::istream& s)
{
    Rc::Number n;
    s.read(reinterpret_cast<char*>(&n), sizeof(n));
    boost::endian::big_to_native_inplace(n);
    return n;
}

static
void write_number(std::ostream& s, Rc::Number n)
{
    boost::endian::native_to_big_inplace(n);
    s.write(reinterpret_cast<char*>(&n), sizeof(n));
}

/* static */
Rc Rc::load(ObjectStore& objects, const ObjectId& id)
{
    using F = fs::fstream;

    auto path = refcount_path(object_path(objects._objdir, id));

    auto f = std::make_unique<fs::fstream>(path, F::binary | F::in | F::out);

    if (!f->is_open()) {
        // File doesn't exist, assume Rc numbers are zero then
        return Rc{objects, id, std::move(path), std::move(f), 0, 0};
    }

    auto n1 = read_number(*f);
    auto n2 = read_number(*f);
    return Rc{objects, id, std::move(path), std::move(f), n1, n2};
}

void Rc::commit()
{
    using F = fs::fstream;

    if (!_file->is_open()) {
        if (fs::exists(_path)) {
            std::cerr << "Rc file created by other Rc object " << _path << "\n";
            ouisync_assert(false); exit(1);
        }
        if (both_are_zero()) return;
        _file->open(_path, F::binary | F::in | F::out | F::trunc);
    }

    if (!_file->is_open()) {
        std::stringstream ss;
        ss << "Failed to commit refcount: " << _path;
        throw std::runtime_error(ss.str());
    }

    if (both_are_zero()) {
        _file->close();
        fs::remove(_path);
        return;
    }

    _file->seekp(0);
    write_number(*_file, _recursive_count);
    write_number(*_file, _direct_count);
}

void Rc::increment_recursive_count() {
    ++_recursive_count;
    commit();
}
void Rc::increment_direct_count()    {
    ++_direct_count;
    commit();
}

void Rc::decrement_recursive_count() {
    ouisync_assert(_recursive_count);
    --_recursive_count;
    commit();

    if (_recursive_count > 0) return;

    auto obj = _objects->load<Tree, Blob::Nothing>(_obj_id);

    apply(obj,
            [&](const Tree& tree) {
                for (auto& id : tree.children()) {
                    Rc::load(*_objects, id).decrement_recursive_count();
                }
            },
            [&](const Blob::Nothing&) {
            });

    if (_direct_count == 0) {
        _objects->remove(_obj_id);
    }
}

void Rc::decrement_recursive_count_but_dont_remove() {
    ouisync_assert(_recursive_count);
    --_recursive_count;
    commit();
}

void Rc::decrement_direct_count(){
    ouisync_assert(_direct_count);
    --_direct_count;
    commit();
    if (!both_are_zero()) return;
    _objects->remove(_obj_id);
}

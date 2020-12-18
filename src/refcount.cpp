#include "refcount.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"
#include "variant.h"
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>
#include <boost/endian/conversion.hpp>

#include <iostream>
#include <sstream>

namespace ouisync::refcount {

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
Number read_number(std::istream& s)
{
    Number n;
    s.read(reinterpret_cast<char*>(&n), sizeof(n));
    boost::endian::big_to_native_inplace(n);
    return n;
}

static
void write_number(std::ostream& s, Number n)
{
    boost::endian::native_to_big_inplace(n);
    s.write(reinterpret_cast<char*>(&n), sizeof(n));
}

// -------------------------------------------------------------------

} // namespace

namespace ouisync {

/* static */
Rc Rc::load(const fs::path& objdir, const ObjectId& id)
{
    using F = fs::fstream;

    auto path = refcount::refcount_path(refcount::object_path(objdir, id));

    auto f = std::make_unique<fs::fstream>(path, F::binary | F::in | F::out);

    if (!f->is_open()) {
        // File doesn't exist, assume Rc numbers are zero then
        return Rc{std::move(path), std::move(f), 0, 0};
    }

    auto n1 = refcount::read_number(*f);
    auto n2 = refcount::read_number(*f);
    return Rc{std::move(path), std::move(f), n1, n2};
}

void Rc::commit()
{
    using F = fs::fstream;

    if (!_file->is_open()) {
        if (fs::exists(_path)) {
            std::cerr << "Rc file created by other Rc object " << _path << "\n";
            assert(false); exit(1);
        }
        _file->open(_path, F::binary | F::in | F::out | F::trunc);
    }

    if (!_file->is_open()) {
        std::stringstream ss;
        ss << "Failed to commit refcount: " << _path;
        throw std::runtime_error(ss.str());
    }

    _file->seekp(0);
    refcount::write_number(*_file, _recursive_count);
    refcount::write_number(*_file, _direct_count);
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
    assert(_recursive_count);
    --_recursive_count;
    commit();
}

void Rc::decrement_direct_count(){
    assert(_direct_count);
    --_direct_count;
    commit();
}

} // namespace

namespace ouisync::refcount {

// -------------------------------------------------------------------
Number increment_recursive(const fs::path& objdir, const ObjectId& id)
{
    auto rc = Rc::load(objdir, id);
    rc.increment_recursive_count();
    return rc.recursive_count();
}

Number read_recursive(const fs::path& objdir, const ObjectId& id) {
    auto rc = Rc::load(objdir, id);
    return rc.recursive_count();
}

// -------------------------------------------------------------------

void flat_remove(const fs::path& objdir, const ObjectId& id) {
    auto rc = Rc::load(objdir, id);
    rc.decrement_direct_count();
    if (!rc.both_are_zero()) return;
    object::io::remove(objdir, id);
}

void deep_remove(const fs::path& objdir, const ObjectId& id) {
    auto rc = Rc::load(objdir, id);
    rc.decrement_recursive_count();

    if (rc.recursive_count() > 0) return;

    auto obj = object::io::load<Tree, Blob::Nothing>(objdir, id);

    apply(obj,
            [&](const Tree& tree) {
                for (auto& [name, id] : tree) {
                    (void)name; // https://stackoverflow.com/a/40714311/273348
                    deep_remove(objdir, id);
                }
            },
            [&](const Blob::Nothing&) {
            });

    if (rc.direct_count() == 0) {
        object::io::remove(objdir, id);
    }
}

// -------------------------------------------------------------------

} // namespace

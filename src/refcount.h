#pragma once

#include "object_id.h"
#include "shortcuts.h"

#include <cstdint>
#include <boost/filesystem/fstream.hpp>

namespace ouisync {

/*
 * We need two counters to keep track of referenced objects: one for when an
 * object is also resposible for preserving all its children, and one for
 * when that is not the case.
 *
 * The former is called `_recursive_count` and the latter is `_direct_count`.
 *
 * Most of the time it is the `_recursive_count` that is interesting. For
 * example, having nodes such as
 *
 *                            N
 *                           🡗 🡖
 *                          M   K
 *
 * then deleting N would decrement the `_recursive_count` and if it ends up
 * being zero, then it tries to delete M and K as well.
 *
 * However, we can only use `_recursive_count` for nodes whose all descendants
 * we have. We call these nodes "complete".
 *
 * We can also have "incomplete" nodes because objects are loaded from the
 * network one by one and we need to preserve these as well. To do so, we
 * increase their `_direct_count`.
 *
 */
class Rc {
  public:
    using Number = uint32_t;

  public:
    static Rc load(const fs::path& objdir, const ObjectId&);

    Number recursive_count() { return _recursive_count; }
    Number direct_count()    { return _direct_count;    }

    void increment_recursive_count();
    void increment_direct_count();

    void decrement_recursive_count();
    void decrement_direct_count();

    bool both_are_zero() const {
        return _recursive_count == 0 && _direct_count == 0;
    }

    friend std::ostream& operator<<(std::ostream& os, const Rc& rc) {
        return os << "Rc{" << rc._recursive_count << ", " << rc._direct_count << "}";
    }

  private:
    Rc(fs::path path, std::unique_ptr<fs::fstream> f, Number k, Number l) :
        _path(std::move(path)),
        _file(std::move(f)),
        _recursive_count(k),
        _direct_count(l)
    {}

    void commit();

  private:
    fs::path _path;
    // fs::fstream doesn't have a move constructor defined in Boost 1.74
    std::unique_ptr<fs::fstream> _file;
    Number _recursive_count;
    Number _direct_count;
};

} // namespace

namespace ouisync::refcount {

using Number = uint32_t;

Number read_recursive(const fs::path& objdir, const ObjectId&);
Number increment_recursive(const fs::path& objdir, const ObjectId&);

void flat_remove(const fs::path& objdir, const ObjectId& id);
void deep_remove(const fs::path& objdir, const ObjectId& id);

} // namespace

#pragma once

#include "shortcuts.h"
#include "ouisync_assert.h"

#include <memory>
#include <array>
#include <streambuf>

namespace ouisync {

class BlockStore;
class ObjectId;

//////////////////////////////////////////////////////////////////////

class Blob {
private:
    struct Impl;

public:
    Blob();

    Blob(const Blob&) = delete;
    Blob& operator=(const Blob&) = delete;

    Blob(Blob&&);
    Blob& operator=(Blob&&);

    static Blob empty(BlockStore&);
    static Blob open(const ObjectId&, BlockStore&);
    static Opt<Blob> maybe_open(const ObjectId&, BlockStore&);

    ObjectId id();

    size_t read(char* buffer, size_t size, size_t offset);
    size_t write(const char* buffer, size_t size, size_t offset);

    void commit();

    size_t size() const;

    ~Blob();

private:
    Blob(std::unique_ptr<Impl> impl);

private:
    std::unique_ptr<Impl> _impl;
};

} // namespace

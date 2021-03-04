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
    static Blob open(const fs::path&, BlockStore&);
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

//////////////////////////////////////////////////////////////////////

class BlobStreamBuffer : public std::streambuf {
public:
    using std::streambuf::char_type;

    BlobStreamBuffer(Blob& blob) :
        _blob(blob), _put_position(0), _get_position(0) {}

    std::streamsize xsputn(const char_type* s, std::streamsize n) override {
        const size_t r = _blob.write(s, n, _put_position);
        _put_position += r;
        return r;
    }

    int_type underflow() override {
        // https://en.cppreference.com/w/cpp/io/basic_streambuf/setg
        if (_get_position == _blob.size()) return traits_type::eof();
        auto r = _blob.read(_read_buf.data(), _read_buf.size(), _get_position);
        setg(_read_buf.data(), _read_buf.data(), _read_buf.data() + r);
        _get_position += r;
        return r;
    }

private:
    Blob& _blob;
    std::array<char, 256> _read_buf;
    size_t _put_position;
    size_t _get_position;
};

//////////////////////////////////////////////////////////////////////

} // namespace

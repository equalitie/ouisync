#pragma once

#include "shortcuts.h"
#include "ouisync_assert.h"

#include <memory>
#include <array>
#include <streambuf>

namespace ouisync {

class BlockStore;
class ObjectId;
class Transaction;

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

    static Blob open(const ObjectId&, const BlockStore&);
    static Blob open(const fs::path&, const BlockStore&);
    static Opt<Blob> maybe_open(const ObjectId&, const BlockStore&);

    ObjectId id();

    size_t read(char* buffer, size_t size, size_t offset);
    size_t write(const char* buffer, size_t size, size_t offset);

    size_t truncate(size_t size);

    void commit(Transaction&);

    size_t size() const;

    ~Blob();

private:
    void maybe_init();

    static std::unique_ptr<Impl> empty_iml();

    Blob(std::unique_ptr<Impl> impl);

private:
    std::unique_ptr<Impl> _impl;
};

//////////////////////////////////////////////////////////////////////

class BlobStreamBuffer : public std::streambuf {
public:
    using std::streambuf::char_type;

    BlobStreamBuffer(Blob& blob) :
        _blob(blob), _put_position(0), _get_position(0), _next_get_position(0) {}

    std::streamsize xsputn(const char_type* s, std::streamsize n) override {
        const size_t r = _blob.write(s, n, _put_position);
        _put_position += r;
        return r;
    }

    int_type underflow() override {
        // https://en.cppreference.com/w/cpp/io/basic_streambuf/setg
        if (_next_get_position == _blob.size()) return traits_type::eof();
        _get_position = _next_get_position;
        auto r = _blob.read(_read_buf.data(), _read_buf.size(), _next_get_position);
        setg(_read_buf.data(), _read_buf.data(), _read_buf.data() + r);
        _next_get_position += r;
        return r;
    }

    size_t getg() const {
        return _get_position + (gptr() - eback());
    }

    size_t getp() const {
        return _put_position;
    }

private:
public:
    Blob& _blob;
    std::array<char, 256> _read_buf;
    size_t _put_position;
    size_t _get_position;
    size_t _next_get_position;
};

//////////////////////////////////////////////////////////////////////

} // namespace

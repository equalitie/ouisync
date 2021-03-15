#pragma once

#include "shortcuts.h"
#include "blob.h"

#include <boost/optional.hpp>

namespace ouisync {

class Transaction;
class BlockId;

struct File
{
public:
    File();

    // The returned File takes ownership iff the `blob` contains
    // the ObjectTag::File tag at the beginning.
    static Opt<File> maybe_open(Blob& blob);

    static File open(Blob& blob)
    {
        Opt<File> f = maybe_open(blob);
        if (!f) {
            throw std::runtime_error("File::open Failed to open blob");
        }
        return std::move(*f);
    }

    BlockId calculate_id();

    size_t write(const char*, size_t size, size_t offset);
    size_t read(char*, size_t size, size_t offset);

    BlockId save(Transaction&);

    size_t size();

    size_t truncate(size_t);

    friend std::ostream& operator<<(std::ostream&, const File&);

private:
    File(Blob&&, size_t);

private:
    Blob _blob;
    size_t _data_offset;
};

} // namespace


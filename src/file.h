#pragma once

#include "object_id.h"
#include "shortcuts.h"

namespace ouisync {

class Blob;
class BlockStore;

struct File : std::vector<uint8_t>
{
public:
    using Parent = std::vector<uint8_t>;
    using std::vector<uint8_t>::vector;

    File(const Parent& p) : Parent(p) {}
    File(Parent&& p) : Parent(std::move(p)) {}

    ObjectId calculate_id() const;

    ObjectId save(BlockStore&) const;
    bool maybe_load(Blob&);

    void load(Blob& blob)
    {
        if (!maybe_load(blob)) {
            throw std::runtime_error("File:: Failed to load from block");
        }
    }

    static size_t read_size(Blob&);

    friend std::ostream& operator<<(std::ostream&, const File&);

private:
};

} // namespace


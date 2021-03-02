#pragma once

#include "object_id.h"
#include "shortcuts.h"
#include "block_store.h"

namespace ouisync {

struct File : std::vector<uint8_t>
{
public:
    using Parent = std::vector<uint8_t>;
    using std::vector<uint8_t>::vector;

    File(const Parent& p) : Parent(p) {}
    File(Parent&& p) : Parent(std::move(p)) {}

    ObjectId calculate_id() const;

    ObjectId save(BlockStore&) const;
    bool maybe_load(const BlockStore::Block&);

    void load(const BlockStore::Block& block)
    {
        if (!maybe_load(block)) {
            throw std::runtime_error("File:: Failed to load from block");
        }
    }

    static
    size_t read_size(const BlockStore::Block&);

    friend std::ostream& operator<<(std::ostream&, const File&);

private:
};

} // namespace


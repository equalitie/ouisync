#include "block_sync.h"
#include <iostream>
#include "variant.h"

#include <hex.h>
#include <array_io.h>

using namespace ouisync;

inline
const char* action_name(const BlockSync::Action& a) {
    return apply(a,
            [] (const BlockSync::ActionModifyBlock&) {
                return "ActionModifyBlock";
            },
            [] (const BlockSync::ActionRemoveBlock&) {
                return "ActionRemoveBlock";
            });
}

void BlockSync::insert_block(const BlockId& block_id, const Digest& digest)
{
    auto [iter, inserted] = _blocks.insert(std::make_pair(block_id, digest));

    if (!inserted) {
        if (iter->second != digest) {
            iter->second = digest;
            _version_vector.increment(_uuid);
        }
    }
}

void BlockSync::remove_block(const BlockId& block_id)
{
    if (_blocks.erase(block_id)) {
        _version_vector.increment(_uuid);
    }
}

void BlockSync::add_action(Action&& action) {
    std::scoped_lock<std::mutex> lock(_mutex);

    apply(action,
        [&] (const BlockSync::ActionModifyBlock& a) {
            std::cerr << action_name(a) << " " << a.id.ToString() << " -> " << to_hex(a.digest) << "\n";
        },
        [&] (const BlockSync::ActionRemoveBlock& a) {
            std::cerr << action_name(a) << " " << a.id.ToString() << "\n";
        });

    return apply(action,
            [&] (const BlockSync::ActionModifyBlock& a) {
                insert_block(a.id, a.digest);
            },
            [&] (const BlockSync::ActionRemoveBlock& a) {
                remove_block(a.id);
            });

}

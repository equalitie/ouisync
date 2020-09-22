#pragma once

#include <boost/variant.hpp>

namespace ouisync {

class BlockSync {
public:
    struct ActionCreateBlock {};
    struct ActionModifyBlock {};
    struct ActionRemoveBlock {};

    using Action = boost::variant<
        ActionCreateBlock,
        ActionModifyBlock,
        ActionRemoveBlock>;

    void add_action(Action&&) {}

private:
};

}

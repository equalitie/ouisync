#pragma once

#include <ouisync/message.hpp>

namespace ouisync {

using HandlerResult = std::variant<std::exception_ptr, Response>;
using HandlerSig = void(HandlerResult);

} // ouisync namespace

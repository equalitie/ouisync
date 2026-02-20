#pragma once

#include <ouisync/message.hpp>

namespace ouisync {

using HandlerResult = std::variant<boost::system::error_code, Response>;
using InnerHandlerSig = void(HandlerResult);
using HandlerSig = void(boost::system::error_code, Response);

template<class Handler>
inline void apply_result(Handler handler, HandlerResult&& result) {
    if (auto* ec = std::get_if<boost::system::error_code>(&result)) {
        handler(*ec, Response{});
    } else {
        handler(boost::system::error_code(), std::get<Response>(std::move(result)));
    }
}

} // ouisync namespace

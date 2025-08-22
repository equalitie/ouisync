#pragma once

#include <ouisync/message.g.hpp>

namespace ouisync {

struct ResponseResult {
    struct Failure {
        error::Service code;
        std::string message;
        std::vector<std::string> sources;
    
        Failure() = default;
    
        Failure(
            error::Service code,
            std::string message,
            std::vector<std::string> sources
        ) :
            code(code),
            message(std::move(message)),
            sources(std::move(sources))
        {}
    };

    using Alternatives = std::variant<
        Response,
        Failure
    >;

    Alternatives value;

    ResponseResult() = default;

    ResponseResult(Alternatives&& v)
        : value(std::forward<Alternatives>(v))
    {}
};

} // namespace ouisync

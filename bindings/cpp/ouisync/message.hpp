#pragma once

#include <ouisync/data.hpp>
#include <ouisync/message.g.hpp>
#include <ouisync/error.hpp>

namespace ouisync {

struct ResponseResult {
    // Errors reported by the Ouisync service
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

        void move_throw() const {
            throw_error(code, std::move(message));
        }

        std::exception_ptr move_to_exception_ptr() {
            try {
                move_throw();
            }
            catch (...) {
                return std::current_exception();
            }
            throw_error(error::logic, "unreachable");
        }
    };

    using Alternatives = std::variant<
        Response,
        Failure
    >;

    Alternatives value;

    ResponseResult() = default;
    ResponseResult(ResponseResult&&) = default;
    ResponseResult(const ResponseResult&) = delete;
    ResponseResult& operator=(const ResponseResult&) = delete;

    ResponseResult(Alternatives&& v)
        : value(std::forward<Alternatives>(v))
    {}
};

} // namespace ouisync

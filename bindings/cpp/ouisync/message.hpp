#pragma once

#include <ouisync/data.hpp>
#include <ouisync/message.g.hpp>
#include <ouisync/error.hpp>
#include <type_traits>

namespace ouisync {

namespace detail {
    template<typename T>
    struct is_map : std::false_type {};

    template<typename K, typename V>
    struct is_map<std::map<K, V>> : std::true_type {};
}

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

/// Given the expected variant, extract the underlying value from the response.
/// Throws `std::bad_variant_access` on variant mismatch.
template<typename Variant>
Variant::type extract(Response response, std::shared_ptr<Client> client) {
    using Type = Variant::type;

    if constexpr (std::is_void_v<Type>) {
        std::get<Variant>(std::move(response.value));
        return;
    } else {
        auto alt = std::get<Variant>(std::move(response.value));

        using ValueType = decltype(alt.value);
        constexpr auto is_api_class = !std::is_same_v<Type, ValueType>;

        if constexpr (is_api_class) {
            if constexpr (detail::is_map<Type>::value) {
                Type out;

                for (auto pair : std::move(alt.value)) {
                    out.emplace(std::move(pair.first), typename Type::mapped_type(client, std::move(pair.second)));
                }

                return out;
            } else {
                // TODO: handle also vector and optional.
                //
                return Type(std::move(client), std::move(alt.value));
            }
        } else {
            return alt.value;
        }
    }
}

} // namespace ouisync

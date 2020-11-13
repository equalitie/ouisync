#pragma once

#include <string>
#include <array>
#include "block_id.h"

#include <boost/optional.hpp>

namespace ouisync {

namespace {
    template<class B> struct is_byte_type { static const bool value = false; };
    
    template<> struct is_byte_type<char> { static const bool value = true; };
    template<> struct is_byte_type<signed char> { static const bool value = true; };
    template<> struct is_byte_type<unsigned char> { static const bool value = true; };
}

template<class OutputT, size_t N, class InputT> std::array<OutputT, 2*N> to_hex(const std::array<InputT, N>& as) noexcept
{
    static_assert(is_byte_type<InputT>::value, "Not a bytestring type");

    std::array<OutputT, 2*N> output;

    const char* digits = "0123456789abcdef";

    for (unsigned int i = 0; i < as.size(); i++) {
        unsigned char c = as.data()[i];
        output[2*i]     = digits[(c >> 4) & 0xf];
        output[2*i + 1] = digits[(c >> 0) & 0xf];
    }

    return output;
}

template<class OutputT, size_t InputSize, class InputT>
inline
std::array<OutputT, InputSize*2> to_hex(const InputT* input) noexcept
{
    static_assert(is_byte_type<InputT>::value, "Not a bytestring type");

    std::array<OutputT, InputSize*2> output;

    const char* digits = "0123456789abcdef";

    for (unsigned int i = 0; i < InputSize; i++) {
        unsigned char c = reinterpret_cast<const unsigned char*>(input)[i];
        output[2*i]   = digits[(c >> 4) & 0xf];
        output[2*i+1] = digits[(c >> 0) & 0xf];
    }

    return output;
}

inline
std::string to_hex(boost::string_view input) noexcept
{
    std::string output;
    output.reserve(input.size() * 2);

    const char* digits = "0123456789abcdef";

    while (!input.empty()) {
        unsigned char c = *reinterpret_cast<const unsigned char*>(&input.front());
        output += digits[(c >> 4) & 0xf];
        output += digits[(c >> 0) & 0xf];
        input.remove_prefix(1);
    }

    return output;
}

inline
boost::optional<unsigned char> from_hex(char c) noexcept
{
    if ('0' <= c && c <= '9') {
        return c - '0';
    } else if ('a' <= c && c <= 'f') {
        return 10 + c - 'a';
    } else if ('A' <= c && c <= 'F') {
        return 10 + c - 'A';
    } else return boost::none;
}

inline
boost::optional<unsigned char> from_hex(char c1, char c2) noexcept
{
    auto on1 = from_hex(c1);
    if (!on1) return boost::none;
    auto on2 = from_hex(c2);
    if (!on2) return boost::none;
    return *on1*16+*on2;
}

inline boost::optional<std::string> from_hex(boost::string_view hex) noexcept
{
    std::string output((hex.size() >> 1) + (hex.size() & 1), '\0');

    size_t i = 0;
    while (size_t s = hex.size()) {
        boost::optional<unsigned char> oc;

        if (s == 1) { oc = from_hex(hex[0]);         hex.remove_prefix(1); }
        else        { oc = from_hex(hex[0], hex[1]); hex.remove_prefix(2); }

        if (!oc) return boost::none;

        output[i++] = *oc;
    }

    return output;
}

template<class OutputT, size_t InputSize>
inline boost::optional<std::array<OutputT, InputSize/2>> from_hex(boost::string_view hex) noexcept
{
    static_assert(InputSize % 2 == 0, "");

    if (InputSize != hex.size()) return boost::none;

    std::array<OutputT, InputSize/2> output;

    size_t i = 0;
    while (size_t s = hex.size()) {
        boost::optional<unsigned char> oc;

        if (s == 1) { oc = from_hex(hex[0]);         hex.remove_prefix(1); }
        else        { oc = from_hex(hex[0], hex[1]); hex.remove_prefix(2); }

        if (!oc) return boost::none;

        output[i++] = *oc;
    }

    return output;
}

template<class OutputT, class InputT, size_t N>
inline boost::optional<std::array<OutputT, N/2>> from_hex(const std::array<InputT, N>& hex) noexcept
{
    static_assert(is_byte_type<InputT>::value, "Not a bytestring type");
    // TODO: This can be generalized to odd number as well
    static_assert(N % 2 == 0, "Input number must have even number of characters");

    std::array<OutputT, N/2> output;

    for (size_t i = 0; i < N; i += 2) {
        boost::optional<unsigned char> oc;
        oc = from_hex(hex[i], hex[i+1]);
        if (!oc) return boost::none;
    }

    return output;
}

} // namespace

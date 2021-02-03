#pragma once

#include "shortcuts.h"

#include <array>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include <boost/optional.hpp>
#include <boost/asio/buffer.hpp>
#include <boost/utility/string_view.hpp>
#include <boost/endian/conversion.hpp>
#include <sha.h> // cryptopp

namespace ouisync {

enum class HashAlgorithm {
    sha1,
    sha256,
    sha512,
};

namespace hash_detail {
    template<HashAlgorithm algo> struct DigestSize;
    template<> struct DigestSize<HashAlgorithm::sha1>   { static constexpr size_t value = 20; };
    template<> struct DigestSize<HashAlgorithm::sha256> { static constexpr size_t value = 32; };
    template<> struct DigestSize<HashAlgorithm::sha512> { static constexpr size_t value = 64; };

    template<HashAlgorithm algo> struct Impl;
    template<> struct Impl<HashAlgorithm::sha1>   { using Type = CryptoPP::SHA1; };
    template<> struct Impl<HashAlgorithm::sha256> { using Type = CryptoPP::SHA256; };
    template<> struct Impl<HashAlgorithm::sha512> { using Type = CryptoPP::SHA512; };

} // namespace hash_detail



/* Templated class to support running hashes.
 *
 * You may call `update` several times to feed the hash function with new
 * data.  When you are done, you may call the `close` function, which returns
 * the resulting digest as an array of bytes.
 */
template<HashAlgorithm algo>
class Hash final {
private:
    using Impl = typename hash_detail::Impl<algo>::Type;

public:
    static constexpr size_t DigestSize = hash_detail::DigestSize<algo>::value;
    using Digest = std::array<uint8_t, DigestSize>;

    Hash()
    {
    }

    inline void update(const void* buffer, size_t size)
    {
        if (!_impl) {
            _impl = Impl();
        }
        _impl->Update((CryptoPP::byte*) buffer, size);
    }

    inline void update(boost::string_view sv)
    {
        update(sv.data(), sv.size());
    }

    inline void update(const char* c)
    {
        update(boost::string_view(c));
    }

    inline void update(const std::string& data)
    {
        update(data.data(), data.size());
    }

    inline void update(boost::asio::const_buffer data)
    {
        update(data.data(), data.size());
    }

    inline void update(const std::vector<unsigned char>& data)
    {
        update(data.data(), data.size());
    }

    template<size_t N>
    inline void update(const std::array<uint8_t, N>& data)
    {
        update(data.data(), N);
    }

    template<class T1, class T2>
    inline void update(const std::pair<T1, T2>& pair)
    {
        update(pair.first);
        update(pair.second);
    }

    template<class T>
    inline
    std::enable_if_t<!std::is_integral<T>::value, void> update(const T& t)
    {
        update_hash(t, *this);
    }

    template<class N>
    inline
    std::enable_if_t<std::is_integral<N>::value, void> update(N n)
    {
        boost::endian::native_to_big_inplace(n);
        update(&n, sizeof(n));
    }

    inline Digest close()
    {
        if (!_impl) {
            _impl = Impl();
        }
        Digest result;
        _impl->Final(result.data());
        _impl = boost::none;
        return result;
    }

    template<class... Args>
    static
    Digest digest(Args&&... args)
    {
        Hash hash;
        return digest_impl(hash, std::forward<Args>(args)...);
    }

    static constexpr size_t size() {
        return DigestSize;
    }

private:

    template<class Hash>
    static
    Digest digest_impl(Hash& hash)
    {
        return hash.close();
    }

    template<class Hash, class Arg, class... Rest>
    static
    Digest digest_impl(Hash& hash, const Arg& arg, const Rest&... rest)
    {
        hash.update(arg);
        return digest_impl(hash, rest...);
    }

private:
    Opt<Impl> _impl;
};

using Sha1   = Hash<HashAlgorithm::sha1>;
using Sha256 = Hash<HashAlgorithm::sha256>;
using Sha512 = Hash<HashAlgorithm::sha512>;

} // namespaces

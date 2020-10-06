#pragma once

#include <array>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include <boost/asio/buffer.hpp>
#include <boost/utility/string_view.hpp>

namespace ouisync {

enum class HashAlgorithm {
    sha1,
    sha256,
    sha512,
};

namespace hash_detail {
    // This was done manually, but there is a static assertion
    // in hash.cpp to make sure this stays correct in case cryptopp
    // changes it.
    template<HashAlgorithm algo> struct ImplSize;
    template<> struct ImplSize<HashAlgorithm::sha1>   { static constexpr size_t value = 208; };
    template<> struct ImplSize<HashAlgorithm::sha256> { static constexpr size_t value = 224; };
    template<> struct ImplSize<HashAlgorithm::sha512> { static constexpr size_t value = 368; };

    template<HashAlgorithm algo> struct DigestSize;
    template<> struct DigestSize<HashAlgorithm::sha1>   { static constexpr size_t value = 20; };
    template<> struct DigestSize<HashAlgorithm::sha256> { static constexpr size_t value = 32; };
    template<> struct DigestSize<HashAlgorithm::sha512> { static constexpr size_t value = 64; };

    void perform_static_assertions();
    void new_impl(HashAlgorithm, void* impl);
    void update_impl(HashAlgorithm, void* impl, const uint8_t* data_in, size_t);
    void close_impl(HashAlgorithm, void* impl, uint8_t* data_out);
    void free_impl(HashAlgorithm, void* impl);

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
    // CryptoPP does an assert (in debug mode) that the data is aligned to 16 bytes.
    static constexpr size_t MEM_ALIGN = 16;
    using ImplBuffer = std::array<uint8_t, hash_detail::ImplSize<algo>::value + MEM_ALIGN>;

public:
    static constexpr size_t DigestSize = hash_detail::DigestSize<algo>::value;
    using Digest = std::array<uint8_t, DigestSize>;

    Hash() :
        _data(_impl.data() + MEM_ALIGN - (((size_t)_impl.data())%MEM_ALIGN))
    {
        hash_detail::perform_static_assertions();
    }

    inline void update(const void* buffer, size_t size)
    {
        if (!_is_set) {
            hash_detail::new_impl(algo, _data);
            _is_set = true;
        }
        hash_detail::update_impl(algo, _data,
                reinterpret_cast<const uint8_t*>(buffer), size);
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

    inline Digest close()
    {
        if (!_is_set) {
            hash_detail::new_impl(algo, _data);
            _is_set = true;
        }
        Digest result;
        hash_detail::close_impl(algo, _data, result.data());
        hash_detail::free_impl(algo, _data);
        _is_set = false;
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

    ~Hash() {
        if (_is_set) {
            hash_detail::free_impl(algo, _data);
        }
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
    bool _is_set = false;
    ImplBuffer _impl;
    uint8_t* _data;
};

using Sha1   = Hash<HashAlgorithm::sha1>;
using Sha256 = Hash<HashAlgorithm::sha256>;
using Sha512 = Hash<HashAlgorithm::sha512>;

} // namespaces

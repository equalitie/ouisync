#include "hash.h"

#include <cryptlib.h>

namespace ouisync { namespace hash_detail {

void perform_static_assertions()
{
    static_assert(sizeof(CryptoPP::SHA1)   == hash_detail::ImplSize<HashAlgorithm::sha1  >::value);
    static_assert(sizeof(CryptoPP::SHA256) == hash_detail::ImplSize<HashAlgorithm::sha256>::value);
    static_assert(sizeof(CryptoPP::SHA512) == hash_detail::ImplSize<HashAlgorithm::sha512>::value);
}

void new_impl(HashAlgorithm algo, void* impl)
{
    switch (algo) {
        case HashAlgorithm::sha1:
            new (impl) CryptoPP::SHA1();
            break;
        case HashAlgorithm::sha256:
            new (impl) CryptoPP::SHA256();
            break;
        case HashAlgorithm::sha512:
            new (impl) CryptoPP::SHA512();
            break;
    }
}

void update_impl(HashAlgorithm algo, void* impl, const uint8_t* data_in, size_t size)
{
    switch (algo) {
        case HashAlgorithm::sha1:
            reinterpret_cast<CryptoPP::SHA1*>(impl)->Update(data_in, size);
            break;
        case HashAlgorithm::sha256:
            reinterpret_cast<CryptoPP::SHA256*>(impl)->Update(data_in, size);
            break;
        case HashAlgorithm::sha512:
            reinterpret_cast<CryptoPP::SHA512*>(impl)->Update(data_in, size);
            break;
    }
}

void close_impl(HashAlgorithm algo, void* impl, uint8_t* data_out)
{
    switch (algo) {
        case HashAlgorithm::sha1:
            reinterpret_cast<CryptoPP::SHA1*>(impl)->Final(data_out);
            break;
        case HashAlgorithm::sha256:
            reinterpret_cast<CryptoPP::SHA256*>(impl)->Final(data_out);
            break;
        case HashAlgorithm::sha512:
            reinterpret_cast<CryptoPP::SHA512*>(impl)->Final(data_out);
            break;
    }
}

void free_impl(HashAlgorithm algo, void* impl) {
    switch (algo) {
        case HashAlgorithm::sha1:
            reinterpret_cast<CryptoPP::SHA1*>(impl)->~SHA1();
            break;
        case HashAlgorithm::sha256:
            reinterpret_cast<CryptoPP::SHA256*>(impl)->~SHA256();
            break;
        case HashAlgorithm::sha512:
            reinterpret_cast<CryptoPP::SHA512*>(impl)->~SHA512();
            break;
    }
}

}} // namespaces

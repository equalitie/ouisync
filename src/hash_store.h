#pragma once

#include "hash.h"
#include "hex.h"
#include "namespaces.h"
#include <boost/filesystem.hpp>

namespace ouisync {

class HashStore {
public:
    using Hash = Sha256;
    using Digest = Hash::Digest;

public:
    //----------------------------------------------------------------
    template<class T, size_t S>
    static
    Opt<Digest> load(const fs::path&, const std::array<T, S>& key);

    template<class T, size_t S>
    static
    bool store(const fs::path&, const std::array<T, S>& key, const Digest&);

    template<class T, size_t S>
    static
    bool erase(const fs::path&, const std::array<T, S>& key);

    //----------------------------------------------------------------
    static
    Opt<Digest> load(const fs::path&, const BlockId& key);

    static
    bool store(const fs::path&, const BlockId& key, const Digest&);

    static
    bool erase(const fs::path&, const BlockId& key);

    //----------------------------------------------------------------

private:
    static
    Opt<Digest> load(const fs::path&, const string_view hex_key);

    static
    bool store(const fs::path&, const string_view hex_key, const string_view hex_digest);

    static
    bool erase(const fs::path&, const string_view hex_key);
};

template<class T, size_t S>
inline
Opt<HashStore::Digest> HashStore::load(const fs::path& path, const std::array<T, S>& key)
{
    auto hex_key = to_hex<char>(key);
    return load(path, string_view{hex_key.data(), hex_key.size()});
}

template<class T, size_t S>
inline
bool HashStore::store(const fs::path& path, const std::array<T, S>& key, const Digest& digest)
{
    auto hex_key = to_hex<char>(key);
    auto hex_digest = to_hex<char>(digest);
    return store(path,
            string_view{hex_key.data(), hex_key.size()},
            string_view{hex_digest.data(), hex_digest.size()});
}

template<class T, size_t S>
inline
bool HashStore::erase(const fs::path& path, const std::array<T, S>& key)
{
    auto hex_key = to_hex<char, BlockId::BINARY_LENGTH>(key.data().data());
    return erase(path, string_view{hex_key.data(), hex_key.size()});
}

inline
Opt<HashStore::Digest> HashStore::load(const fs::path& path, const BlockId& key)
{
    auto hex_key = to_hex<char, BlockId::BINARY_LENGTH>((char*)key.data().data());
    return load(path, string_view{hex_key.data(), hex_key.size()});
}

template<class T> struct Foo;
inline
bool HashStore::store(const fs::path& path, const BlockId& key, const Digest& digest)
{
    auto hex_key = to_hex<char, BlockId::BINARY_LENGTH>((char*)key.data().data());
    auto hex_digest = to_hex<char>(digest);
    //Foo<decltype(hex_digest)> d;
    return store(path,
            string_view{hex_key.data(), hex_key.size()},
            string_view{hex_digest.data(), hex_digest.size()});
}

inline
bool HashStore::erase(const fs::path& path, const BlockId& key)
{
    auto hex_key = to_hex<char, BlockId::BINARY_LENGTH>(key.data().data());
    return erase(path, string_view{hex_key.data(), hex_key.size()});
}

} // namespace

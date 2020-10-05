#include <hash_store.h>
#include <boost/filesystem.hpp>
#include "hex.h"

using namespace ouisync;

namespace {
    struct Entry {
        string_view hex_key;
        string_view hex_digest;
    };
}

static Opt<Entry> _read_entry(string_view& input, size_t key_size) {
    Entry entry;
    if (input.size() < key_size) return boost::none;
    entry.hex_key = string_view(input.data(), key_size);
    if (!input.starts_with(' ')) return boost::none;
    input.remove_prefix(1);
    const size_t digest_hex_size = HashStore::Hash::DigestSize * 2;
    if (input.size() < digest_hex_size) return boost::none;
    entry.hex_digest = string_view(input.data(), digest_hex_size);
    if (!input.starts_with('\n')) return boost::none;
    input.remove_prefix(1); 
    return entry;
}

template<class Stream>
static std::string _read_file(Stream& f)
{
    return std::string(std::istreambuf_iterator<char>(f), std::istreambuf_iterator<char>());
}

/* static */
Opt<HashStore::Digest> HashStore::load(const fs::path& path, const string_view hex_key)
{
    fs::ifstream file(path);
    if (!file.is_open()) return boost::none;

    std::string data = _read_file(file);
    string_view data_sw(data);

    while (auto entry = _read_entry(data_sw, hex_key.size())) {
        if (entry->hex_key == hex_key) {
            return from_hex<uint8_t, Hash::DigestSize*2>(entry->hex_digest);
        }
    }

    return boost::none;
}

/* static */
bool HashStore::store(const fs::path& path, const string_view hex_key, const string_view hex_digest)
{
    fs::fstream file(path, fs::fstream::out);
    if (!file.is_open()) return false;

    std::string data = _read_file(file);
    string_view data_sw(data);

    Opt<fs::fstream::pos_type> entry_pos;

    while (auto entry = _read_entry(data_sw, hex_key.size())) {
        if (entry->hex_key == hex_key) {
            entry_pos = entry->hex_key.data() - data.data();
            break;
        }
    }

    if (entry_pos) {
        file.seekp(*entry_pos);
    } else {
        file.seekp(0, std::ios_base::end);
    }

    file << hex_key << " " << hex_digest << "\n";

    return true;
}

/* static */
bool HashStore::erase(const fs::path& path, const string_view hex_key)
{
    fs::fstream file(path);
    if (!file.is_open()) return false;

    std::string data = _read_file(file);
    string_view data_sw(data);

    Opt<fs::fstream::pos_type> entry_pos;

    while (auto entry = _read_entry(data_sw, hex_key.size())) {
        if (entry->hex_key == hex_key) {
            entry_pos = entry->hex_key.data() - data.data();
            break;
        }
    }

    if (!entry_pos) false;

    file.seekp(*entry_pos);
    std::fill_n(std::ostream_iterator<char>(file), hex_key.size(), '0');
    file << ' ';
    std::fill_n(std::ostream_iterator<char>(file), Hash::DigestSize * 2, '0');
    file << '\n';

    return true;
}

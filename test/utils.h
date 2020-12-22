#pragma once

#include "shortcuts.h"
#include "random.h"

#include <random>

struct Random {
    using ObjectId = ouisync::ObjectId;
    using Blob = ouisync::object::Blob;
    using Seed = std::mt19937::result_type;

    static Seed seed() {
        return std::random_device()();
    }

    Random() : gen(seed()) {}

    std::vector<uint8_t> vector(size_t size) {
        std::vector<uint8_t> v(size);
        auto ptr = static_cast<uint8_t*>(v.data());
        fill(ptr, size);
        return v;
    }

    Blob blob(size_t size) {
        return {vector(size)};
    }

    std::string string(size_t size) {
        std::string result(size, '\0');
        fill(&result[0], size);
        return result;
    }

    ObjectId object_id() {
        ObjectId id;
        fill(reinterpret_cast<char*>(id.data()), id.size);
        return id;
    }

    void fill(void* ptr, size_t size) {
        std::uniform_int_distribution<> distrib(0, 255);
        for (size_t i = 0; i < size; ++i) ((char*)ptr)[i] = distrib(gen);
    }

    std::mt19937 gen;
};

struct MainTestDir {
    MainTestDir(std::string name) {
        root = ouisync::fs::unique_path("/tmp/ouisync/test-" + name + "-%%%%-%%%%");
    }

    ouisync::fs::path subdir(std::string name) const {
        return root / name;
    }

    ouisync::fs::path root;
};

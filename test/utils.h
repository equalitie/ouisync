#pragma once

#include "shortcuts.h"
#include "random.h"

#include <random>

struct Random {
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

    std::string string(size_t size) {
        std::string result(size, '\0');
        fill(&result[0], size);
        return result;
    }

    void fill(void* ptr, size_t size) {
        std::uniform_int_distribution<> distrib(0, 255);
        for (size_t i = 0; i < size; ++i) ((char*)ptr)[i] = distrib(gen);
    }

    std::mt19937 gen;
};

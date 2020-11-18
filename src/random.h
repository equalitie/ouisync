#pragma once

#include <cstddef> // size_t

namespace ouisync::random {

void generate_blocking(unsigned char*, size_t);
void generate_non_blocking(unsigned char*, size_t);

}

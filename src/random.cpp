#include "random.h"

#include <osrng.h> // cryptopp

namespace ouisync::random {

void generate_blocking(unsigned char* output, size_t size)
{
    CryptoPP::OS_GenerateRandomBlock(true, output, size);
}

void generate_non_blocking(unsigned char* output, size_t size)
{
    CryptoPP::OS_GenerateRandomBlock(false, output, size);
}

} // ouisync::random namespace

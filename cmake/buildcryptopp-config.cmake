project(BuildCryptoPP VERSION 1.0.0 LANGUAGES CXX)

include(FetchContent)
fetchcontent_populate(CryptoPP URL https://github.com/weidai11/cryptopp/archive/CRYPTOPP_8_2_0.tar.gz)
fetchcontent_populate(CryptoPPCMake URL https://github.com/noloader/cryptopp-cmake/archive/CRYPTOPP_8_2_0.tar.gz)

set(cryptopp_cmake_srcdir ${CMAKE_CURRENT_BINARY_DIR}/cryptoppcmake-src)
set(cryptopp_srcdir ${CMAKE_CURRENT_BINARY_DIR}/cryptopp-src)

file(COPY ${cryptopp_cmake_srcdir}/CMakeLists.txt DESTINATION ${cryptopp_srcdir})

add_subdirectory(${cryptopp_srcdir})

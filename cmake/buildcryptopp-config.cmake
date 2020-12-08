project(BuildCryptoPP VERSION 1.0.0 LANGUAGES CXX)

include(FetchContent)
fetchcontent_populate(CryptoPP URL https://github.com/weidai11/cryptopp/archive/CRYPTOPP_8_2_0.tar.gz)
fetchcontent_populate(CryptoPPCMake URL https://github.com/noloader/cryptopp-cmake/archive/CRYPTOPP_8_2_0.tar.gz)

set(cryptopp_cmake_srcdir ${CMAKE_CURRENT_BINARY_DIR}/cryptoppcmake-src)
set(cryptopp_srcdir ${CMAKE_CURRENT_BINARY_DIR}/cryptopp-src)

file(COPY ${cryptopp_cmake_srcdir}/CMakeLists.txt DESTINATION ${cryptopp_srcdir})

option(BUILD_SHARED "" OFF)
option(BUILD_TESTING "" OFF)

if(ANDROID)
    include_directories(${ANDROID_NDK}/sources/android/cpufeatures)
endif()

add_subdirectory(${cryptopp_srcdir} ${cryptopp_srcdir}/binaries)

if(ANDROID)
    add_library(CryptoPP STATIC ${ANDROID_NDK}/sources/android/cpufeatures/cpu-features.c)
    target_link_libraries(CryptoPP PUBLIC cryptopp-static)
    target_include_directories(CryptoPP PUBLIC ${cryptopp_srcdir})
else()
    add_library(CryptoPP ALIAS cryptopp-static)
endif()

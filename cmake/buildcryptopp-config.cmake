project(BuildCryptoPP VERSION 1.0.0 LANGUAGES CXX)

include(FetchContent)
fetchcontent_populate(CryptoPP URL https://github.com/weidai11/cryptopp/archive/CRYPTOPP_8_2_0.tar.gz)

set(cryptopp_srcdir ${CMAKE_CURRENT_BINARY_DIR}/cryptopp-src)
set(cryptopp_lib ${CMAKE_CURRENT_BINARY_DIR}/cryptopp-src/libcryptopp.a)
set(cryptopp_flags "CXX=${CMAKE_CXX_COMPILER}")

if(CMAKE_CXX_COMPILER_ID STREQUAL "Clang")
    set(cryptopp_flags "${cryptopp_flags} -stdlib=libc++")
else()
    set(cryptopp_flags "${cryptopp_flags}")
endif()

if(CMAKE_GENERATOR MATCHES "Makefiles")
    set(MAKE_CMD $(MAKE))
else()
    set(MAKE_CMD make)
endif()

# XXX: Using both add_custom_command and add_custom_target seem to be faster
# than just running $(MAKE) directly from add_custom_command. Not sure why.
add_custom_command(
    OUTPUT ${cryptopp_lib}
    COMMAND ${MAKE_CMD} libcryptopp.a ${cryptopp_flags}
    WORKING_DIRECTORY ${cryptopp_srcdir})

add_custom_target(CryptoPP_Target DEPENDS ${cryptopp_lib})

#add_custom_target(CryptoPP_Target
#    $(MAKE) libcryptopp.a ${cryptopp_flags}
#    WORKING_DIRECTORY ${cryptopp_srcdir})

add_library(CryptoPP STATIC IMPORTED)
add_dependencies(CryptoPP CryptoPP_Target)
set_target_properties(CryptoPP
    PROPERTIES
    IMPORTED_LOCATION ${cryptopp_lib}
    INTERFACE_INCLUDE_DIRECTORIES ${cryptopp_srcdir})

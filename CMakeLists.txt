cmake_minimum_required(VERSION 3.15)

# Useful cmake info
# https://pabloariasal.github.io/2018/02/19/its-time-to-do-cmake-right/

project(OuiSync)

if(CMAKE_CXX_COMPILER_ID STREQUAL "Clang")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fcoroutines-ts -stdlib=libc++ -std=c++17")
else()
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fcoroutines -std=c++2a -DBOOST_ASIO_HAS_STD_COROUTINE")
endif()

set(CMAKE_THREAD_PREFER_PTHREAD TRUE)
set(THREADS_PREFER_PTHREAD_FLAG TRUE)
find_package(Threads REQUIRED)

set(BuildBoost_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake/build-boost")
find_package(BuildBoost REQUIRED)

set(BuildCryptoPP_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
find_package(BuildCryptoPP REQUIRED)

# 2021-01-29:
# On Android, the sanitizer causes the linker to fail.
# The solution may be to make sure to compile all the libraries using the sanitizer flag, but
# it requires verification.
if (NOT ANDROID)
    set (CMAKE_CXX_FLAGS_DEBUG "${CMAKE_CXX_FLAGS_DEBUG} -fno-omit-frame-pointer -fsanitize=address")
    set (CMAKE_LINKER_FLAGS_DEBUG "${CMAKE_LINKER_FLAGS_DEBUG} -fno-omit-frame-pointer -fsanitize=address")
else()
    set (CMAKE_CXX_FLAGS_DEBUG "${CMAKE_CXX_FLAGS_DEBUG} -fno-omit-frame-pointer")
    set (CMAKE_LINKER_FLAGS_DEBUG "${CMAKE_LINKER_FLAGS_DEBUG} -fno-omit-frame-pointer")
endif()

function(set_warning_level TARGET)
    if(MSVC)
        target_compile_options(${TARGET} PRIVATE /W4 /WX)
    else()
        target_compile_options(${TARGET} PRIVATE -Wall -Wextra -Werror)
        if (CMAKE_BUILD_TYPE STREQUAL "Debug")
            target_compile_options(${TARGET} PRIVATE -Wno-unused-parameter -Wno-unused-private-field)
        endif()
    endif()
endfunction(set_warning_level)

add_library(libouisync STATIC
    "src/user_id.cpp"
    "src/branch.cpp"
    "src/branch_view.cpp"
    "src/repository.cpp"
    "src/file_system_attrib.cpp"
    "src/options.cpp"
    "src/network.cpp"
    "src/message_broker.cpp"
    "src/client.cpp"
    "src/server.cpp"
    "src/message.cpp"
    "src/random.cpp"
    "src/commit.cpp"
    "src/object_id.cpp"
    "src/object_tag.cpp"
    "src/object_store.cpp"
    "src/directory.cpp"
    "src/file_blob.cpp"
    "src/index.cpp"
    "src/version_vector.cpp"
)

target_compile_definitions(libouisync PRIVATE -DBOOST_ASIO_HAS_CO_AWAIT)
target_include_directories(libouisync PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src)
target_link_libraries(libouisync PUBLIC Boost::filesystem Boost::serialization Boost::program_options CryptoPP)
#target_compile_features(libouisync PRIVATE cxx_std_17)
set_warning_level(libouisync)

if(NOT ANDROID)
    add_executable(ouisync src/ouisync.cpp src/fuse_runner.cpp)
    target_include_directories(ouisync PRIVATE /usr/include)
    target_compile_definitions(ouisync PRIVATE -D_FILE_OFFSET_BITS=64)
    target_link_libraries(ouisync PRIVATE libouisync fuse)

    add_subdirectory(test)
endif()

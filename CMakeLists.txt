cmake_minimum_required(VERSION 3.15)

# Useful cmake info
# https://pabloariasal.github.io/2018/02/19/its-time-to-do-cmake-right/

project(OuiSync)

set(CMAKE_THREAD_PREFER_PTHREAD TRUE)
set(THREADS_PREFER_PTHREAD_FLAG TRUE)
find_package(Threads REQUIRED)

set(BuildBoost_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake/build-boost")
find_package(BuildBoost REQUIRED)

# Dependency config for CryFS, it is basically a copy of the one in
# modules/cryfs/cmake-utils/DependenciesFromConan.cmake but changed to use
# Boost found through the above find_package instead of the one from Conan.
set(DEPENDENCY_CONFIG
    "${CMAKE_CURRENT_SOURCE_DIR}/cmake/cryfs-deps.cmake"
    CACHE FILEPATH
    "cmake configuration file defining how to get dependencies"
)

add_subdirectory(modules/cryfs EXCLUDE_FROM_ALL)

# This will set up include directories for cryfs when included by
# target_link_libraries.
set_target_properties(cryfs PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/modules/cryfs/src"
)

add_library(ouisync STATIC
    "src/create_cry_device.cpp"
    "src/block_store.cpp"
    "src/hash.cpp"
    "src/block_sync.cpp"
    "src/object/any.cpp"
    "src/object/tree.cpp"
    "src/object/block.cpp"
    "src/object/path.cpp"
)
target_include_directories(ouisync PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src)
target_link_libraries(ouisync PUBLIC Boost::filesystem Boost::serialization cryfs)
target_compile_features(ouisync PRIVATE cxx_std_17)

add_subdirectory(test)

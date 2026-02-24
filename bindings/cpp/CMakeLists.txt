cmake_minimum_required(VERSION 3.10)
project(Ouisync)

include(FetchContent)

# Based on the value of this constant, this example will either create a new
# Ouisync service instance to which the client will connect (ON), or the
# client will attempt to connect to a Ouisync app already running on this PC
# (OFF).
option(EXAMPLE_USE_BUILT_IN_SERVICE "When OFF the example will attempt to connect to an already running Ouisync app" ON)
option(OUISYNC_MEASURE_BUILD_TIME "Print target build duration" OFF)

option(WARNING_IS_ERROR "Fail compilation in presence of warnings" OFF)
option(OUISYNC_BOOST_ASIO_SEPARATE_COMPILATION OFF)
# Enable mounting on Linux and Windows, requires FUSE and Dokan respectively.
option(OUISYNC_WITH_VFS "Compile with Virtual File System support" OFF)

# Show compilation times
if (OUISYNC_MEASURE_BUILD_TIME)
    set_property(GLOBAL PROPERTY RULE_LAUNCH_COMPILE "${CMAKE_CURRENT_SOURCE_DIR}/cmake/measure_build_time.sh")
    set_property(GLOBAL PROPERTY RULE_LAUNCH_LINK "${CMAKE_CURRENT_SOURCE_DIR}/cmake/measure_build_time.sh")
endif()

# Enable compilation warnings for all targets
if (MSVC)
    add_compile_options(/W4 /WX)
else()
    add_compile_options(-Wall -Wextra -Wpedantic)
    if (WARNING_IS_ERROR)
        add_compile_options(-Werror)
    endif()
endif()

###########################################################
# Boost
###########################################################

# TODO: Get rid of this
# https://stackoverflow.com/a/71864590/273348
set(Boost_NO_WARN_NEW_VERSIONS ON)

# If you need to specify path to custom Boost installation, use `cmake
# -D<VAR>=<VAL>` in the _configure_ stage where `<VAR>` is either `BOOST_ROOT`
# or the pair of `BOOST_INCLUDEDIR` and `BOOST_LIBRARYDIR`.
# https://cmake.org/cmake/help/v3.0/module/FindBoost.html
find_package(Boost
    # Boost.Hash2 needed for HMAC had initial release in 1.88.0
    1.88.0 REQUIRED
    COMPONENTS context filesystem
)

FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.2
)

###########################################################
# msgpack - for serializing messages between C++ and Rust
###########################################################

file(WRITE ${CMAKE_BINARY_DIR}/msgpack/download/CMakeLists.txt
    "cmake_minimum_required(VERSION 2.8.2)\n"
    "project(msgpack-download NONE)\n"
    ""
    "include(ExternalProject)\n"
    "externalproject_add(msgpack\n"
    "  GIT_REPOSITORY    https://github.com/msgpack/msgpack-c\n"
    "  # Use commit from the `cpp_master` which contains the msgpack-cxx C++ version\n"
    "  GIT_TAG           092bc69b6e815980bce7808595c914dd3a29f905\n"
    "  SOURCE_DIR        ${CMAKE_BINARY_DIR}/msgpack/src\n"
    "  BINARY_DIR        ${CMAKE_BINARY_DIR}/msgpack/build\n"
    "  # No building, that's done outside of this externalproject_add\n"
    "  CONFIGURE_COMMAND \"\"\n"
    "  BUILD_COMMAND     \"\"\n"
    "  INSTALL_COMMAND   \"\"\n"
    "  TEST_COMMAND      \"\"\n"
    ")"
)

# Configure
execute_process(
    COMMAND "${CMAKE_COMMAND}" -G "${CMAKE_GENERATOR}" .
    WORKING_DIRECTORY "${CMAKE_BINARY_DIR}/msgpack/download"
)

# Build
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build .
    WORKING_DIRECTORY "${CMAKE_BINARY_DIR}/msgpack/download"
)

# Import targets
add_subdirectory("${CMAKE_BINARY_DIR}/msgpack/src" "msgpack" EXCLUDE_FROM_ALL)

###########################################################
# Ouisync Rust code
###########################################################

FetchContent_MakeAvailable(Corrosion)

# CMAKE_CURRENT_SOURCE_DIR points to the dir path of this CMakeLists.txt file
set(OUISYNC_SRC_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/../../)

set(OUISYNC_SERVICE_PRIVATE_LIBS)

if (WIN32)
    list(APPEND OUISYNC_SERVICE_PRIVATE_LIBS netio iphlpapi propsys mincore)
endif()

if (OUISYNC_WITH_VFS)
    set(OUISYNC_CARGO_FEATURES "vfs")
    list(APPEND OUISYNC_SERVICE_PRIVATE_LIBS fuse3) # TODO: Windows uses Dokan
endif()

# This creates the 'ouisync_service' library target whose sources are written in Rust
corrosion_import_crate(
    MANIFEST_PATH ${OUISYNC_SRC_ROOT}/Cargo.toml
    # Can also create a dynamic library if crate type is set to `cdylib`.
    CRATE_TYPES staticlib
    CRATES ouisync-service
    FEATURES ${OUISYNC_CARGO_FEATURES}
)

# TODO: This doesn't seem to do anything apart from preventing
# `externalproject_add` that adds this project from failing when its
# `INSTALL_COMMAND` is left unset.
install(TARGETS ouisync_service DESTINATION ${CMAKE_INSTALL_PREFIX})

add_library(cpp_ouisync_service
    STATIC
        ouisync/service.cpp
)

target_link_libraries(cpp_ouisync_service
    PUBLIC
        ouisync_service
        # Required when using Boost.Asio with stackful coroutines
        Boost::context
        # For reading ouisync config files
        Boost::filesystem
        msgpack-cxx
    PRIVATE
        ${OUISYNC_SERVICE_PRIVATE_LIBS}
)

target_include_directories(cpp_ouisync_service
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}
    PRIVATE
        ${Boost_INCLUDE_DIRS}
)

if (OUISYNC_BOOST_ASIO_SEPARATE_COMPILATION)
    target_compile_definitions(cpp_ouisync_service PUBLIC -DBOOST_ASIO_SEPARATE_COMPILATION)
endif()

###########################################################
# C++ API code generation
###########################################################

set(OUISYNC_GENERATED_BINDING_DIR ${CMAKE_CURRENT_SOURCE_DIR}/ouisync)
# To avoid re-building every time, the generated files are first written into a
# temporary directory and then copied to where the compiler can find them only
# if they have not changed.
set(OUISYNC_GENERATED_BINDING_TMP_DIR ${CMAKE_CURRENT_BINARY_DIR}/ouisync/generated.tmp)

set(GENERATED_CPP_FILES
    ${OUISYNC_GENERATED_BINDING_DIR}/data.g.cpp
    ${OUISYNC_GENERATED_BINDING_DIR}/message.g.cpp
)

set(GENERATED_HPP_FILES
    ${OUISYNC_GENERATED_BINDING_DIR}/api.g.hpp
    ${OUISYNC_GENERATED_BINDING_DIR}/data.g.hpp
    ${OUISYNC_GENERATED_BINDING_DIR}/data_dsc.g.hpp
    ${OUISYNC_GENERATED_BINDING_DIR}/message.g.hpp
    ${OUISYNC_GENERATED_BINDING_DIR}/message_dsc.g.hpp
)

add_custom_target(ouisync_generate_api
    COMMAND
        mkdir -p ${OUISYNC_GENERATED_BINDING_TMP_DIR} &&
        cargo run --package ouisync-bindgen -- cpp ${OUISYNC_GENERATED_BINDING_TMP_DIR} &&
        ${CMAKE_COMMAND} -E copy_directory_if_different
            ${OUISYNC_GENERATED_BINDING_TMP_DIR}/
            ${OUISYNC_GENERATED_BINDING_DIR}
    WORKING_DIRECTORY
        ${OUISYNC_SRC_ROOT}
    BYPRODUCTS
        ${GENERATED_CPP_FILES}
        ${GENERATED_HPP_FILES}
)

add_library(cpp_ouisync_client
    STATIC
        ouisync/client.cpp
        ouisync/serialize.cpp
        ouisync/error.cpp
        ouisync/subscription.cpp
        ouisync/subscriptions.cpp
        ${GENERATED_CPP_FILES}
)

target_include_directories(cpp_ouisync_client
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}
    PRIVATE
        ${Boost_INCLUDE_DIRS}
)

target_link_libraries(cpp_ouisync_client
    PUBLIC
        # Required when using Boost.Asio with stackful coroutines
        Boost::context
        # For reading ouisync config files
        Boost::filesystem
        msgpack-cxx
)

target_compile_features(cpp_ouisync_client PUBLIC cxx_std_20)
target_compile_features(cpp_ouisync_service PUBLIC cxx_std_20)

if (OUISYNC_BOOST_ASIO_SEPARATE_COMPILATION)
    target_compile_definitions(cpp_ouisync_client PUBLIC -DBOOST_ASIO_SEPARATE_COMPILATION)
endif()

if (NOT OUISYNC_BOOST_ASIO_SEPARATE_COMPILATION)
    ############################
    # Example
    ############################
    add_executable(example examples/main.cpp)

    if(EXAMPLE_USE_BUILT_IN_SERVICE)
        target_compile_definitions(example PUBLIC USE_BUILT_IN_SERVICE)
        target_link_libraries(example PUBLIC cpp_ouisync_client cpp_ouisync_service)
    else()
        target_link_libraries(example PUBLIC cpp_ouisync_client)
    endif()

    ############################
    # Tests
    ############################
    enable_testing()
    add_subdirectory(tests)
endif()

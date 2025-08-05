# The Flutter tooling requires that developers have CMake 3.10 or later
# installed. You should not increase this version, as doing so will cause
# the plugin to fail to compile for some customers of the plugin.
cmake_minimum_required(VERSION 3.10)

# Project-level configuration.
set(PROJECT_NAME "logtee")
project(${PROJECT_NAME})

file(REAL_PATH ${CMAKE_CURRENT_SOURCE_DIR} RESOLVED_CMAKE_CURRENT_SOURCE_DIR)
set(ROOT_DIR "${RESOLVED_CMAKE_CURRENT_SOURCE_DIR}/../..")

include(FetchContent)

FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.1
)
# Set any global configuration variables such as `Rust_TOOLCHAIN` before this line!
FetchContent_MakeAvailable(Corrosion)

corrosion_import_crate(
  MANIFEST_PATH "${ROOT_DIR}/rust/Cargo.toml"
  CRATES logtee
)

# List of absolute paths to libraries that should be bundled with the plugin.
# This list could contain prebuilt libraries, or libraries created by an
# external build triggered from this build file.
set(logtee_bundled_libraries
  "${CMAKE_CURRENT_BINARY_DIR}/liblogtee.so"
  PARENT_SCOPE
)

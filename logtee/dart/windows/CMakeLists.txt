# The Flutter tooling requires that developers have CMake 3.10 or later
# installed. You should not increase this version, as doing so will cause
# the plugin to fail to compile for some customers of the plugin.
cmake_minimum_required(VERSION 3.10)

# Project-level configuration.
set(PROJECT_NAME "logtee")
project(${PROJECT_NAME})

# Resolve the symlink in CMAKE_CURRENT_SOURCE_DIR using powershell because
# `file(REAL_PATH)` doesn't work on Windows or is not yet fixed in the `cmake`
# version 3.31.6-msvc6 bundled with Visual Studio 17.
# https://gitlab.kitware.com/cmake/cmake/-/issues/17206
execute_process(
    COMMAND powershell.exe -Command "(Get-Item ${CMAKE_CURRENT_SOURCE_DIR}).Target"
    OUTPUT_VARIABLE RESOLVED_CMAKE_CURRENT_SOURCE_DIR)

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
  "${CMAKE_CURRENT_BINARY_DIR}/logtee.dll"
  PARENT_SCOPE
)

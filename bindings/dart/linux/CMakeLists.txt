# The Flutter tooling requires that developers have CMake 3.10 or later
# installed. You should not increase this version, as doing so will cause
# the plugin to fail to compile for some customers of the plugin.

# The Corrosion package for building Rust works better with cmake >= 3.19
# https://github.com/corrosion-rs/corrosion
cmake_minimum_required(VERSION 3.19)

# Project-level configuration.

set(PROJECT_NAME "ouisync")

project(${PROJECT_NAME} LANGUAGES CXX)

include(FetchContent)

# CMAKE_SOURCE_DIR points to `.../ouisync-app/linux` (absolute path)
file(CREATE_LINK ${CMAKE_SOURCE_DIR}/../ouisync ${CMAKE_SOURCE_DIR}/../ouisync/bindings/dart/ouisync SYMBOLIC)

FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.1 # Optionally specify a commit hash, version tag or branch here
)
# Set any global configuration variables such as `Rust_TOOLCHAIN` before this line!
FetchContent_MakeAvailable(Corrosion)

set(OUISYNC_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../ouisync")

corrosion_import_crate(
  MANIFEST_PATH "${OUISYNC_DIR}/Cargo.toml"
  CRATES ouisync-service
)

# This value is used when generating builds using this plugin, so it must
# not be changed.
set(PLUGIN_NAME "ouisync_plugin")

# Define the plugin library target. Its name must not be changed (see comment
# on PLUGIN_NAME above).
#
# Any new source files that you add to the plugin should be added here.
add_library(${PLUGIN_NAME} SHARED
  "ouisync_plugin.cc"
)

# Apply a standard set of build settings that are configured in the
# application-level CMakeLists.txt. This can be removed for plugins that want
# full control over build settings.
apply_standard_settings(${PLUGIN_NAME})

# Symbols are hidden by default to reduce the chance of accidental conflicts
# between plugins. This should not be removed; any symbols that should be
# exported should be explicitly exported with the FLUTTER_PLUGIN_EXPORT macro.
set_target_properties(${PLUGIN_NAME} PROPERTIES
  CXX_VISIBILITY_PRESET hidden)
target_compile_definitions(${PLUGIN_NAME} PRIVATE FLUTTER_PLUGIN_IMPL)

# Source include directories and library dependencies. Add any plugin-specific
# dependencies here.
target_include_directories(${PLUGIN_NAME} INTERFACE
  "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_link_libraries(${PLUGIN_NAME} PRIVATE flutter)
target_link_libraries(${PLUGIN_NAME} PRIVATE PkgConfig::GTK)
target_link_libraries(${PLUGIN_NAME} PUBLIC ouisync_service)

# List of absolute paths to libraries that should be bundled with the plugin.
# This list could contain prebuilt libraries, or libraries created by an
# external build triggered from this build file.
set(ouisync_bundled_libraries "${CMAKE_CURRENT_BINARY_DIR}/libouisync_service.so"
  PARENT_SCOPE
)

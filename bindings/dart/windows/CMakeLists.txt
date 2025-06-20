# We need to use cmake version >= 3.19 to be able to use FetchContent and include Corrosion for building Rust
# https://github.com/corrosion-rs/corrosion
cmake_minimum_required(VERSION 3.19)

set(PROJECT_NAME "ouisync")

project(${PROJECT_NAME} LANGUAGES CXX)

include(FetchContent)

# CMAKE_SOURCE_DIR points to `.../ouisync-app/windows` (absolute path)
# For some reason, this doesn't work if the link is relative.
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

# Not putting it inside Debug or Release subdirectories because they don't
# necessarily exist yet. Or maybe we should create it?
set(DOKAN_DLL_OUTPUT_PATH "${CMAKE_CURRENT_BINARY_DIR}")

# This forces the rust-dokan dependency to create the dokan2.dll library and
# place in the ${binary_output_dir} directory.
corrosion_set_env_vars(ouisync_service "DOKAN_DLL_OUTPUT_PATH=${DOKAN_DLL_OUTPUT_PATH}")

# This value is used when generating builds using this plugin, so it must
# not be changed
set(PLUGIN_NAME "ouisync_plugin")

add_library(${PLUGIN_NAME} SHARED
  "ouisync_plugin.cpp"
)
apply_standard_settings(${PLUGIN_NAME})
set_target_properties(${PLUGIN_NAME} PROPERTIES
  CXX_VISIBILITY_PRESET hidden)
target_compile_definitions(${PLUGIN_NAME} PRIVATE FLUTTER_PLUGIN_IMPL)
target_include_directories(${PLUGIN_NAME} INTERFACE
  "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_link_libraries(${PLUGIN_NAME} PRIVATE flutter flutter_wrapper_plugin)

# List of absolute paths to libraries that should be bundled with the plugin
set(ouisync_bundled_libraries
  ${CMAKE_CURRENT_BINARY_DIR}/ouisync_service.dll
  ${DOKAN_DLL_OUTPUT_PATH}/dokan2.dll
  PARENT_SCOPE
)

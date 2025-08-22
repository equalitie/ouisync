cmake_minimum_required(VERSION 3.5)

include(FetchContent)

FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.2
)

FetchContent_MakeAvailable(Corrosion)

# CMAKE_CURRENT_SOURCE_DIR points to the dir path of this CMakeLists.txt file
set(OUISYNC_SRC_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/../../)

corrosion_import_crate(
    MANIFEST_PATH ${OUISYNC_SRC_ROOT}/Cargo.toml
    # Can also create a dynamic library if crate type is set to `cdylib`.
    CRATE_TYPES staticlib
    CRATES ouisync-service
)

# TODO: This doesn't seem to do anything apart from preventing
# `externalproject_add` that adds this project from failing when its
# `INSTALL_COMMAND` is left unset.
install(TARGETS ouisync_service DESTINATION ${CMAKE_INSTALL_PREFIX})

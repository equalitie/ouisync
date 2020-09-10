include(cmake-utils/conan.cmake)

conan_cmake_run(
    CONANFILE conanfile.py
    BUILD missing)

conan_basic_setup(TARGETS SKIP_STD NO_OUTPUT_DIRS)

add_library(CryfsDependencies_range-v3 INTERFACE)
target_link_libraries(CryfsDependencies_range-v3 INTERFACE CONAN_PKG::range-v3)

add_library(CryfsDependencies_spdlog INTERFACE)
target_link_libraries(CryfsDependencies_spdlog INTERFACE CONAN_PKG::spdlog)

# Setup boost dependency
set(Boost_USE_STATIC_LIBS OFF)
find_package(Boost REQUIRED)
add_library(CryfsDependencies_boost INTERFACE)
target_link_libraries(CryfsDependencies_boost INTERFACE Boost::boost Boost::filesystem Boost::thread Boost::chrono Boost::program_options)
if(${CMAKE_SYSTEM_NAME} MATCHES "Linux")
    # Also link to rt, because boost thread needs that.
    target_link_libraries(CryfsDependencies_boost INTERFACE rt)
endif()

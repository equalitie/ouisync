
project(CryFSTests)

add_executable(test-cryfs "cryfs.cpp")
target_link_libraries(test-cryfs  Threads::Threads ouisync)
target_compile_features(test-cryfs PRIVATE cxx_std_17)
target_include_directories(test-cryfs PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)

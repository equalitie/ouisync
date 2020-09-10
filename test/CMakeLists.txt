
project(CryFSTests)

add_executable(test-cryfs "cryfs.cpp")
target_link_libraries(test-cryfs  Threads::Threads cryfs)
target_compile_features(test-cryfs PRIVATE cxx_std_17)

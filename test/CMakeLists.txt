project(CryFSTests)

add_executable(test-cryfs "cryfs.cpp")
target_link_libraries(test-cryfs  Threads::Threads Boost::unit_test_framework ouisync)
target_compile_features(test-cryfs PRIVATE cxx_std_17)
target_include_directories(test-cryfs PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)
set_warning_level(test-cryfs)

add_executable(objcat "objcat.cpp")
target_link_libraries(objcat Threads::Threads ouisync)
target_compile_features(objcat PRIVATE cxx_std_17)
target_include_directories(objcat PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)
set_warning_level(objcat)

add_executable(test-version-vector "version-vector.cpp")
target_link_libraries(test-version-vector PUBLIC Boost::unit_test_framework)
set_warning_level(test-version-vector)

add_executable(test-objects "objects.cpp")
target_link_libraries(test-objects PUBLIC Boost::unit_test_framework ouisync)
target_compile_features(test-objects PRIVATE cxx_std_17)
set_warning_level(test-objects)

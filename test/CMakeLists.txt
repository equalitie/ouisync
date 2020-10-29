project(Tests)

add_executable(objcat "objcat.cpp")
target_link_libraries(objcat Threads::Threads ouisync)
target_compile_features(objcat PRIVATE cxx_std_17)
target_include_directories(objcat PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)
set_warning_level(objcat)

add_executable(test-version-vector "version-vector.cpp")
target_link_libraries(test-version-vector PUBLIC Boost::unit_test_framework)
set_warning_level(test-version-vector)

add_executable(test-objects "test-objects.cpp")
target_link_libraries(test-objects PUBLIC Boost::unit_test_framework ouisync)
target_compile_features(test-objects PRIVATE cxx_std_17)
set_warning_level(test-objects)

add_executable(test-file test-file.cpp ../src/file.cpp ../src/file_locker.cpp)
target_link_libraries(test-file PUBLIC Boost::unit_test_framework Boost::filesystem "${CMAKE_THREAD_LIBS_INIT}")
target_compile_definitions(test-file PRIVATE -DBOOST_ASIO_HAS_CO_AWAIT)
target_compile_features(test-file PRIVATE cxx_std_17)
set_warning_level(test-file)

add_executable(test-utils test-utils.cpp)
target_link_libraries(test-utils PUBLIC Boost::unit_test_framework "${CMAKE_THREAD_LIBS_INIT}")
target_compile_definitions(test-utils PRIVATE -DBOOST_ASIO_HAS_CO_AWAIT)
target_compile_features(test-utils PRIVATE cxx_std_17)
set_warning_level(test-utils)

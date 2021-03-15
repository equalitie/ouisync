project(Tests)

add_executable(objcat "objcat.cpp")
target_link_libraries(objcat Threads::Threads libouisync)
target_compile_features(objcat PRIVATE cxx_std_17)
target_include_directories(objcat PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)
set_warning_level(objcat)

add_executable(test-version-vector "version-vector.cpp" "${CMAKE_CURRENT_SOURCE_DIR}/../src/user_id.cpp")
target_link_libraries(test-version-vector PUBLIC Boost::unit_test_framework Boost::filesystem)
target_include_directories(test-version-vector PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)
set_warning_level(test-version-vector)

add_executable(test-utils test-utils.cpp)
target_link_libraries(test-utils PUBLIC Boost::unit_test_framework "${CMAKE_THREAD_LIBS_INIT}")
target_compile_definitions(test-utils PRIVATE -DBOOST_ASIO_HAS_CO_AWAIT)
target_compile_features(test-utils PRIVATE cxx_std_17)
set_warning_level(test-utils)

######################################################################
add_executable(test-blob test-blob.cpp
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/block_store.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/transaction.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/index.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/user_id.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/version_vector.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/block_id.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/blob.cpp")

target_include_directories(test-blob PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../src)

target_link_libraries(test-blob PUBLIC Boost::unit_test_framework
    CryptoPP
    Boost::filesystem
    "${CMAKE_THREAD_LIBS_INIT}")

target_compile_features(test-blob PRIVATE cxx_std_17)
set_warning_level(test-blob)

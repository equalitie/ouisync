set(tests cancellation
          copy
          dht
          network_sockets
          serialize
          subscriptions)

foreach(test IN LISTS tests)
    add_executable("test_${test}" "test_${test}.cpp" test_utils.cpp)
    target_link_libraries("test_${test}" PRIVATE cpp_ouisync_client cpp_ouisync_service)
    add_test(NAME ${test} COMMAND "test_${test}")
endforeach()

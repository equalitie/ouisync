add_executable(test_copy test_copy.cpp test_utils.cpp)
target_link_libraries(test_copy PRIVATE cpp_ouisync_client cpp_ouisync_service)
add_test(NAME copy COMMAND test_copy)

add_executable(test_dht test_dht.cpp test_utils.cpp)
target_link_libraries(test_dht PRIVATE cpp_ouisync_client cpp_ouisync_service)
add_test(NAME dht COMMAND test_dht)

add_executable(test_network_sockets test_network_sockets.cpp test_utils.cpp)
target_link_libraries(test_network_sockets PRIVATE cpp_ouisync_client cpp_ouisync_service)
add_test(NAME network_sockets COMMAND test_network_sockets)

add_executable(test_serialize test_serialize.cpp)
target_link_libraries(test_serialize PRIVATE cpp_ouisync_client)
add_test(NAME serialize COMMAND test_serialize)

add_executable(test_subscriptions test_subscriptions.cpp test_utils.cpp)
target_link_libraries(test_subscriptions PRIVATE cpp_ouisync_client cpp_ouisync_service)
add_test(NAME subscriptions COMMAND test_subscriptions)

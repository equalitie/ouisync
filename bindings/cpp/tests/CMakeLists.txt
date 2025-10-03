add_executable(
    test_subscriptions
    test_subscriptions.cpp
)
target_link_libraries(
    test_subscriptions
    PRIVATE
        cpp_ouisync_client
)

# Register in CTest
add_test(
    NAME OuisyncTests
    COMMAND test_subscriptions
)

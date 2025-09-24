cmake_minimum_required(VERSION 3.10)

project(OuisyncCppExample VERSION 1.0)

# Based on the value of this constant, this example will either create a new
# Ouisync service instance to which the client will connect (ON), or the
# client will attempt to connect to a Ouisync app already running on this PC
# (OFF).
option(USE_BUILT_IN_SERVICE "When OFF the example will attempt to connect to an already running Ouisync app" ON)

add_subdirectory("../" "ouisync" EXCLUDE_FROM_ALL)

if(USE_BUILT_IN_SERVICE)
    set(LINKED_LIBRARIES cpp_ouisync_client cpp_ouisync_service)
else()
    set(LINKED_LIBRARIES cpp_ouisync_client)
endif()

add_executable(example main.cpp)
target_link_libraries(example ${LINKED_LIBRARIES})

if(USE_BUILT_IN_SERVICE)
    target_compile_definitions(example PUBLIC USE_BUILT_IN_SERVICE)
endif()

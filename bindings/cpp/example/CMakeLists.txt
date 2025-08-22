cmake_minimum_required(VERSION 3.10)

project(OuisyncCppExample VERSION 1.0)

add_subdirectory("../" "ouisync" EXCLUDE_FROM_ALL)

add_executable(example main.cpp)
target_link_libraries(example ouisync_client)

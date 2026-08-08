cmake_minimum_required(VERSION 3.25)

if(NOT TARGET libslic3r OR NOT TARGET nlohmann_json)
  message(FATAL_ERROR "orca-bridge must be added after OrcaSlicer src and deps_src")
endif()

add_executable(orca-bridge bridge.cpp)
target_compile_features(orca-bridge PRIVATE cxx_std_20)
target_include_directories(orca-bridge PRIVATE "${CMAKE_SOURCE_DIR}/src")
target_link_libraries(orca-bridge PRIVATE libslic3r nlohmann_json)

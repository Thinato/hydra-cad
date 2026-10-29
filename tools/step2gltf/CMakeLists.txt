cmake_minimum_required(VERSION 3.16)
project(step2gltf CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Release)
endif()

find_package(OpenCASCADE REQUIRED)
message(STATUS "OpenCASCADE ${OpenCASCADE_VERSION}")

# Imported targets pull in their own dependencies, so only the top-level toolkits are named.
set(OCCT_LIBS TKPrim TKBO TKMesh TKXCAF TKRWMesh)
if(TARGET TKDESTEP)
  # OCCT 7.8 merged the STEP toolkits into TKDESTEP and moved glTF into TKDEGLTF.
  list(APPEND OCCT_LIBS TKDESTEP TKDEGLTF)
else()
  list(APPEND OCCT_LIBS TKXDESTEP)
endif()

foreach(prog step2gltf make_fixtures)
  add_executable(${prog} ${prog}.cpp)
  target_include_directories(${prog} SYSTEM PRIVATE ${OpenCASCADE_INCLUDE_DIR})
  target_link_libraries(${prog} PRIVATE ${OCCT_LIBS})
  if(MSVC)
    target_compile_options(${prog} PRIVATE /W4)
  else()
    target_compile_options(${prog} PRIVATE -Wall -Wextra)
  endif()
endforeach()

install(TARGETS step2gltf make_fixtures)

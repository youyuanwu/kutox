cmake_minimum_required(VERSION 3.24)
project(kutox NONE)

# Remove obsolete generated registrations when reusing an older build directory.
file(REMOVE "${CMAKE_BINARY_DIR}/CTestTestfile.cmake" "${CMAKE_BINARY_DIR}/DartConfiguration.tcl")

find_package(Git 2.36 REQUIRED)
execute_process(
    COMMAND "${GIT_EXECUTABLE}" rev-parse HEAD:linux
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    OUTPUT_VARIABLE LINUX_REVISION OUTPUT_STRIP_TRAILING_WHITESPACE
    COMMAND_ERROR_IS_FATAL ANY
)
add_custom_target(linux-source
    COMMAND "${GIT_EXECUTABLE}" submodule init linux
    COMMAND "${GIT_EXECUTABLE}" init linux
    COMMAND "${GIT_EXECUTABLE}" -C linux config remote.origin.url https://github.com/gregkh/linux.git
    COMMAND "${GIT_EXECUTABLE}" -C linux sparse-checkout set --cone
        drivers/net/ethernet/intel/e1000 include arch/x86/include
    COMMAND "${GIT_EXECUTABLE}" -C linux fetch --depth 1 --filter=blob:none origin "${LINUX_REVISION}"
    COMMAND "${GIT_EXECUTABLE}" -C linux checkout --detach "${LINUX_REVISION}"
    COMMAND "${GIT_EXECUTABLE}" submodule absorbgitdirs linux
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    USES_TERMINAL
    VERBATIM
)
option(KUTOX_SOURCE_ONLY "Configure only Linux source preparation" OFF)
if(KUTOX_SOURCE_ONLY)
    return()
endif()

find_program(CARGO cargo REQUIRED)
find_program(XORRISO xorriso REQUIRED)
find_package(Python3 REQUIRED COMPONENTS Interpreter)

include(FetchContent)
FetchContent_Declare(limine
    URL https://github.com/Limine-Bootloader/Limine/releases/download/v12.0.2/limine-binary.tar.gz?download=1
    URL_HASH SHA256=3ceded87b8edee7c61085a06b5d3afe8bf371bf639498165dcf0f4b89b05dbfc
    DOWNLOAD_EXTRACT_TIMESTAMP ON
)
FetchContent_MakeAvailable(limine)

set(KERNEL "${CMAKE_SOURCE_DIR}/target/x86_64-unknown-none/release/kutox")
option(KUTOX_GUEST_TESTS "Include guest acceptance fixtures in the reusable image" ON)
if(KUTOX_GUEST_TESTS)
    set(KERNEL_FEATURES --features guest-tests)
    set(IMAGE_PROFILE ON)
else()
    set(KERNEL_FEATURES --no-default-features)
    set(IMAGE_PROFILE OFF)
endif()
add_custom_target(kernel
    COMMAND ${CARGO} build --locked --release --jobs 1 ${KERNEL_FEATURES}
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    BYPRODUCTS "${KERNEL}"
    USES_TERMINAL
)

add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/kutox.iso"
    BYPRODUCTS "${CMAKE_BINARY_DIR}/kutox.iso.manifest.json"
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
        --kernel "${KERNEL}" --limine "${limine_SOURCE_DIR}"
        --output "${CMAKE_BINARY_DIR}/kutox.iso" --xorriso "${XORRISO}"
        --profile "${IMAGE_PROFILE}"
    DEPENDS kernel "${KERNEL}" "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
    VERBATIM
)
add_custom_target(images DEPENDS "${CMAKE_BINARY_DIR}/kutox.iso")

add_custom_target(linux-driver-check
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/linux_driver_check.py"
        --build-dir "${CMAKE_BINARY_DIR}"
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    USES_TERMINAL
    VERBATIM
)

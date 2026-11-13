cmake_minimum_required(VERSION 3.24)
project(kutox NONE)

# Remove obsolete generated registrations when reusing an older build directory.
file(REMOVE "${CMAKE_BINARY_DIR}/CTestTestfile.cmake" "${CMAKE_BINARY_DIR}/DartConfiguration.tcl")

find_program(CARGO cargo REQUIRED)
find_program(XORRISO xorriso REQUIRED)
find_program(CLANG clang REQUIRED)
find_program(LLD ld.lld REQUIRED)
find_program(MKE2FS mke2fs REQUIRED)
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
    OUTPUT "${CMAKE_BINARY_DIR}/init.elf"
    COMMAND ${CLANG} --target=x86_64-unknown-none -c
        "${CMAKE_SOURCE_DIR}/userspace/init.S" -o "${CMAKE_BINARY_DIR}/init.o"
    COMMAND ${LLD} --build-id=none --fatal-warnings --script
        "${CMAKE_SOURCE_DIR}/userspace/linker.ld" -o "${CMAKE_BINARY_DIR}/init.elf"
        "${CMAKE_BINARY_DIR}/init.o"
    DEPENDS userspace/init.S userspace/linker.ld
    VERBATIM
)
add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/rootfs.img"
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/make_rootfs.py"
        --program "${CMAKE_BINARY_DIR}/init.elf" --output "${CMAKE_BINARY_DIR}/rootfs.img"
        --mke2fs "${MKE2FS}"
    DEPENDS "${CMAKE_BINARY_DIR}/init.elf" scripts/make_rootfs.py
    VERBATIM
)
add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/kutox.iso"
    BYPRODUCTS "${CMAKE_BINARY_DIR}/kutox.iso.manifest.json"
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
        --kernel "${KERNEL}" --limine "${limine_SOURCE_DIR}"
        --output "${CMAKE_BINARY_DIR}/kutox.iso" --xorriso "${XORRISO}"
        --rootfs "${CMAKE_BINARY_DIR}/rootfs.img"
        --profile "${IMAGE_PROFILE}"
    DEPENDS kernel "${KERNEL}" "${CMAKE_BINARY_DIR}/rootfs.img" "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
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

cmake_minimum_required(VERSION 3.24)
project(kutox NONE)
enable_testing()

find_program(CARGO cargo REQUIRED)
find_program(XORRISO xorriso REQUIRED)
find_program(QEMU qemu-system-x86_64 REQUIRED)
find_program(NM nm REQUIRED)
find_package(Python3 REQUIRED COMPONENTS Interpreter)

include(FetchContent)
FetchContent_Declare(limine
    URL https://github.com/Limine-Bootloader/Limine/releases/download/v12.0.2/limine-binary.tar.gz
    URL_HASH SHA256=3ceded87b8edee7c61085a06b5d3afe8bf371bf639498165dcf0f4b89b05dbfc
    DOWNLOAD_EXTRACT_TIMESTAMP ON
)
FetchContent_MakeAvailable(limine)

set(KERNEL "${CMAKE_SOURCE_DIR}/target/x86_64-unknown-none/release/kutox")
add_custom_target(kernel
    COMMAND ${CARGO} build --locked --release --jobs 1
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    BYPRODUCTS "${KERNEL}"
    USES_TERMINAL
)

add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/kutox.iso"
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
        --kernel "${KERNEL}" --limine "${limine_SOURCE_DIR}"
        --output "${CMAKE_BINARY_DIR}/kutox.iso" --xorriso "${XORRISO}"
    DEPENDS kernel "${KERNEL}" "${CMAKE_SOURCE_DIR}/scripts/make_iso.py"
    VERBATIM
)
add_custom_target(images DEPENDS "${CMAKE_BINARY_DIR}/kutox.iso")

foreach(case IN ITEMS known alternate missing unsupported ambiguous eeprom-timeout low-memory default-mode invalid-mode page-fault general-protection double-fault unexpected-page-fault smp-unsupported repeat provenance)
    add_test(NAME "mac-${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 20
    )
    set_tests_properties("mac-${case}" PROPERTIES TIMEOUT 65 RUN_SERIAL TRUE)
endforeach()
add_test(NAME kernel-suite
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 20
)
add_test(NAME kernel-suite-rejection
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests-fail --expect-test-failure --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 20
)
add_test(NAME kernel-suite-low-memory
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests-low-memory --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 20
)
set_tests_properties(kernel-suite kernel-suite-rejection kernel-suite-low-memory PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
add_test(NAME heap-oom-rejection
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case heap-oom --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 20
)
set_tests_properties(heap-oom-rejection PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
add_test(NAME thread-stack-guard
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case thread-stack-guard --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 20
)
set_tests_properties(thread-stack-guard PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
add_test(NAME harness-unit
    COMMAND ${Python3_EXECUTABLE} -m unittest discover -s tests -v
)
set_tests_properties(harness-unit PROPERTIES WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}")
add_test(NAME moss-libkernel-unit
    COMMAND ${CARGO} test --locked -p moss-libkernel --target x86_64-unknown-linux-gnu
)
set_tests_properties(moss-libkernel-unit PROPERTIES
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    TIMEOUT 65
    RUN_SERIAL TRUE
)
add_test(NAME boot-config-unit
    COMMAND ${CARGO} test --locked -p kutox --lib --target x86_64-unknown-linux-gnu
)
set_tests_properties(boot-config-unit PROPERTIES
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    TIMEOUT 65
    RUN_SERIAL TRUE
)

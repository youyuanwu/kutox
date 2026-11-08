cmake_minimum_required(VERSION 3.24)
project(kutox NONE)
enable_testing()

find_program(CARGO cargo REQUIRED)
find_program(XORRISO xorriso REQUIRED)
find_program(QEMU qemu-system-x86_64 REQUIRED)
find_program(NM nm REQUIRED)
find_program(LINUX_DRIVER_CLANG clang REQUIRED)
find_program(LINUX_DRIVER_LD ld REQUIRED)
find_package(Python3 REQUIRED COMPONENTS Interpreter)

include(FetchContent)
FetchContent_Declare(limine
    URL https://github.com/Limine-Bootloader/Limine/releases/download/v12.0.2/limine-binary.tar.gz
    URL_HASH SHA256=3ceded87b8edee7c61085a06b5d3afe8bf371bf639498165dcf0f4b89b05dbfc
    DOWNLOAD_EXTRACT_TIMESTAMP ON
)
FetchContent_MakeAvailable(limine)

set(KERNEL "${CMAKE_SOURCE_DIR}/target/x86_64-unknown-none/release/kutox")
option(KUTOX_GUEST_TESTS "Include guest acceptance fixtures in the reusable image" ON)
if(KUTOX_GUEST_TESTS)
    set(KERNEL_FEATURES --features guest-tests)
    set(GUEST_TEST_PROFILE enabled)
else()
    set(KERNEL_FEATURES --no-default-features)
    set(GUEST_TEST_PROFILE disabled)
endif()
add_custom_target(kernel
    COMMAND ${CARGO} build --locked --release --jobs 1 ${KERNEL_FEATURES}
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

add_custom_target(linux-driver-check
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/linux_driver_check.py"
        --build-dir "${CMAKE_BINARY_DIR}" --clang "${LINUX_DRIVER_CLANG}" --ld "${LINUX_DRIVER_LD}"
    WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
    USES_TERMINAL
    VERBATIM
)
add_test(NAME linux-driver-compile-check
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/linux_driver_check.py"
        --build-dir "${CMAKE_BINARY_DIR}" --clang "${LINUX_DRIVER_CLANG}" --ld "${LINUX_DRIVER_LD}"
)
set_tests_properties(linux-driver-compile-check PROPERTIES TIMEOUT 65 RUN_SERIAL TRUE)

set(MAC_CASES known alternate missing unsupported ambiguous low-memory invalid-mode smp-unsupported repeat provenance)
if(KUTOX_GUEST_TESTS)
    list(APPEND MAC_CASES eeprom-timeout page-fault general-protection double-fault unexpected-page-fault)
endif()
foreach(case IN LISTS MAC_CASES)
    add_test(NAME "mac-${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 20
    )
    set_tests_properties("mac-${case}" PROPERTIES TIMEOUT 65 RUN_SERIAL TRUE)
endforeach()
add_test(NAME kernel-build-profile
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/guest_build_check.py"
        --kernel "${KERNEL}" --nm "${NM}" --guest-tests "${GUEST_TEST_PROFILE}"
)
foreach(case IN ITEMS driver-runtime-default driver-runtime-run driver-runtime-driver-run driver-runtime-default-no-nic driver-runtime-default-unsupported
        network-run network-run-low-memory network-run-no-nic network-run-unsupported)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 20
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
endforeach()
if(KUTOX_GUEST_TESTS)
add_test(NAME kernel-suite
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 80
)
add_test(NAME kernel-suite-rejection
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests-fail --expect-test-failure --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 80
)
add_test(NAME kernel-suite-low-memory
    COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
        --case kernel-tests-low-memory --build-dir "${CMAKE_BINARY_DIR}"
        --qemu "${QEMU}" --timeout 80
)
set_tests_properties(kernel-suite kernel-suite-rejection kernel-suite-low-memory PROPERTIES TIMEOUT 85 RUN_SERIAL TRUE)
foreach(topology IN ITEMS bridge nested bridge-low-memory nested-low-memory)
    add_test(NAME "kernel-suite-${topology}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "kernel-tests-${topology}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 80
    )
    set_tests_properties("kernel-suite-${topology}" PROPERTIES TIMEOUT 85 RUN_SERIAL TRUE)
endforeach()
foreach(case IN ITEMS driver-runtime-root driver-runtime-bridge driver-runtime-nested driver-runtime-low-memory driver-runtime-bridge-low-memory driver-runtime-nested-low-memory driver-runtime-no-nic driver-runtime-no-nic-low-memory driver-runtime-unsupported driver-runtime-unsupported-low-memory)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 25
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 30 RUN_SERIAL TRUE)
endforeach()
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
foreach(case IN ITEMS driver-runtime-tests driver-runtime-tests-low-memory)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 20
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
endforeach()
foreach(case IN ITEMS async-core-tests async-core-tests-low-memory)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 25
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 30 RUN_SERIAL TRUE)
endforeach()
foreach(case IN ITEMS async-network-tests async-network-tests-low-memory async-network-tests-nested-low-memory)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 35
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 40 RUN_SERIAL TRUE)
endforeach()
foreach(case IN ITEMS driver-network-tests driver-network-tests-low-memory driver-network-tests-ambiguous)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 25
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 30 RUN_SERIAL TRUE)
endforeach()
foreach(case IN ITEMS driver-hardening-tests driver-hardening-tests-low-memory driver-hardening-tests-nested-low-memory)
    add_test(NAME "${case}"
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case "${case}" --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --nm "${NM}" --timeout 75
    )
    set_tests_properties("${case}" PROPERTIES TIMEOUT 80 RUN_SERIAL TRUE)
endforeach()
else()
    add_test(NAME guest-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case guest-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 20
    )
    set_tests_properties(guest-tests-disabled PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
    add_test(NAME driver-runtime-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case driver-runtime-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 20
    )
    set_tests_properties(driver-runtime-tests-disabled PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
    add_test(NAME driver-network-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case driver-network-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 20
    )
    set_tests_properties(driver-network-tests-disabled PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
    add_test(NAME driver-hardening-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case driver-hardening-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 20
    )
    set_tests_properties(driver-hardening-tests-disabled PROPERTIES TIMEOUT 25 RUN_SERIAL TRUE)
    add_test(NAME async-core-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case async-core-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 25
    )
    set_tests_properties(async-core-tests-disabled PROPERTIES TIMEOUT 30 RUN_SERIAL TRUE)
    add_test(NAME async-network-tests-disabled
        COMMAND ${Python3_EXECUTABLE} "${CMAKE_SOURCE_DIR}/scripts/qemu_test.py"
            --case async-network-tests-disabled --build-dir "${CMAKE_BINARY_DIR}"
            --qemu "${QEMU}" --timeout 35
    )
    set_tests_properties(async-network-tests-disabled PROPERTIES TIMEOUT 40 RUN_SERIAL TRUE)
endif()
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

/* Limine layout, following embclox/examples-kernel/linker.ld. */
ENTRY(kmain)

PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0xffffffff80000000;
    _kernel_start = .;
    .text : {
        _kernel_text_start = .;
        /* Production services exist before default boot activation. */
        KEEP(*(.text.kutox_async_executor))
        KEEP(*(.text.kutox_packet_broker))
        KEEP(*(.text.kutox_user_bringup))
        KEEP(*(.text.kutox_user_load_elf))
        KEEP(*(.text.kutox_user_dispatch_next))
        *(.text .text.*)
        _kernel_text_end = .;
    } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata
    .kutox.driver : {
        KEEP(*(.kutox.module_init))
        KEEP(*(.kutox.module_exit))
        KEEP(*(.kutox.parameters))
        KEEP(*(.kutox.parameter_descriptions))
        KEEP(*(.kutox.device_tables))
    } :rodata
    .kutox.compile_only : { KEEP(*(.kutox.compile_only)) } :rodata
    ASSERT(SIZEOF(.kutox.compile_only) == 0,
           "compile-only Linux driver objects are not a runtime driver")
    .kutox.native_inits ALIGN(8) : {
        _native_init_header = .;
        KEEP(*(.kutox.native_init_header))
        _native_init_header_end = .;
        _native_init_production_start = .;
        KEEP(*(.kutox.native_init.production))
        _native_init_production_end = .;
        _native_init_test_start = .;
        KEEP(*(.kutox.native_init.test))
        _native_init_test_end = .;
    } :rodata
    ASSERT(_native_init_header_end - _native_init_header == 16,
           "missing or duplicate native initializer header")
    ASSERT((_native_init_production_end - _native_init_production_start) % 24 == 0,
           "malformed production initializer layout")
    ASSERT((_native_init_test_end - _native_init_test_start) % 24 == 0,
           "malformed test initializer layout")
    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :data
    . = ALIGN(4096);
    .bss : { *(.bss .bss.*) *(COMMON) } :data
    . = ALIGN(4096);
    _kernel_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.note*) }
}

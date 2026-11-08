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
    .text : { *(.text .text.*) } :text
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

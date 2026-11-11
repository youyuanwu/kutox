ENTRY(_start)
PHDRS {
    headers PT_LOAD FILEHDR PHDRS FLAGS(4);
    text PT_LOAD FLAGS(5);
    data PT_LOAD FLAGS(6);
    stack PT_GNU_STACK FLAGS(6);
}
SECTIONS {
    . = 0x100000000000 + SIZEOF_HEADERS;
    .headers : { BYTE(0) } :headers
    . = 0x100000001080;
    .text : { *(.text*) *(.rodata*) } :text
    . = 0x100000003ff0;
    .data : { *(.data*) } :data
    .bss : { *(.bss*) } :data
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}

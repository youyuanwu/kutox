.intel_syntax noprefix
.section .text,"ax",@progbits
.global _start
_start:
    test rsp, 15
    jnz .Lbad
    cmp qword ptr [rsp], 2
    jne .Lbad
    mov rax, [rsp + 8]
    cmp dword ptr [rax], 0x666c65
    jne .Lbad
    mov rax, [rsp + 16]
    cmp dword ptr [rax], 0x65646f63
    je .Lcode_fault
    cmp dword ptr [rax], 0x61746164
    je .Ldata_fault
    cmp dword ptr [rax], 0x706f6f6c
    je .Lforever
    cmp dword ptr [rax], 0x006e7572
    jne .Lbad
    cmp qword ptr [rsp + 24], 0
    jne .Lbad
    mov rax, [rsp + 32]
    cmp dword ptr [rax], 0x3d59454b
    jne .Lbad
    cmp dword ptr [rax + 4], 0x756c6176
    jne .Lbad
    cmp word ptr [rax + 8], 0x65
    jne .Lbad
    cmp qword ptr [rsp + 40], 0
    jne .Lbad
    cmp qword ptr [rsp + 48], 5
    jne .Lbad
    cmp qword ptr [rsp + 56], 4
    jne .Lbad
    cmp qword ptr [rsp + 64], 4
    jne .Lbad
    cmp qword ptr [rsp + 72], 56
    jne .Lbad
    cmp qword ptr [rsp + 80], 9
    jne .Lbad
    lea rax, [rip + _start]
    cmp [rsp + 88], rax
    jne .Lbad
    cmp qword ptr [rsp + 96], 3
    jne .Lbad
    mov rax, 0x100000000040
    cmp [rsp + 104], rax
    jne .Lbad
    cmp dword ptr [rax], 1
    jne .Lbad
    cmp qword ptr [rsp + 112], 6
    jne .Lbad
    cmp qword ptr [rsp + 120], 4096
    jne .Lbad
    cmp qword ptr [rsp + 128], 0
    jne .Lbad
    cmp qword ptr [rsp + 136], 0
    jne .Lbad
    lea rsi, [rip + initialized]
    xor ecx, ecx
.Ldata_loop:
    lea eax, [rcx + 1]
    cmp [rsi + rcx], al
    jne .Lbad
    inc ecx
    cmp ecx, 32
    jne .Ldata_loop
    lea rsi, [rip + zeroed]
    mov ecx, 8192
.Lbss_loop:
    cmp byte ptr [rsi], 0
    jne .Lbad
    inc rsi
    dec ecx
    jnz .Lbss_loop
    mov byte ptr [rip + zeroed], 0xa5
    mov byte ptr [rip + zeroed + 8191], 0x5a
    mov byte ptr [rip + initialized], 0xff
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, 23
    int 0x80
    cmp rax, 23
    jne .Lbad
    mov edi, 44
    jmp .Lexit
.Lcode_fault:
    mov byte ptr [rip + _start], 0
    ud2
.Ldata_fault:
    lea rax, [rip + initialized]
    jmp rax
.Lforever:
    jmp .Lforever
.Lbad:
    mov edi, 99
.Lexit:
    mov eax, 60
    int 0x80
    ud2
.section .rodata,"a",@progbits
message:
    .ascii "# USER_ELF_WRITE hello\n"
.section .data,"aw",@progbits
initialized:
    .byte 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16
    .byte 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
.section .bss,"aw",@nobits
zeroed:
    .skip 8192

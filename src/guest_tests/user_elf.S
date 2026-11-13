.intel_syntax noprefix
.section .text,"ax",@progbits
.global _start
_start:
    test rsp, 15
    jnz .Lbad
    cmp qword ptr [rsp], 3
    je .Ltask
    cmp qword ptr [rsp], 2
    jne .Lbad
    mov rax, [rsp + 8]
    cmp dword ptr [rax], 0x666c65
    jne .Lbad
    mov rax, [rsp + 16]
    cmp dword ptr [rax], 0x75706e69
    je .Linput
    cmp dword ptr [rax], 0x65636e6f
    je .Lsleep_once
    cmp dword ptr [rax], 0x65656c73
    je .Lsleep_forever
    cmp dword ptr [rax], 0x6c656979
    je .Lyield_forever
    cmp dword ptr [rax], 0x65746167
    je .Lblocking_gate
    cmp dword ptr [rax], 0x62646162
    je .Lbad_blocking
    cmp dword ptr [rax], 0x746f7571
    je .Lquota
    cmp dword ptr [rax], 0x65646f63
    je .Lcode_fault
    cmp dword ptr [rax], 0x61746164
    je .Ldata_fault
    cmp dword ptr [rax], 0x706f6f6c
    je .Lforever
    cmp dword ptr [rax], 0x73646162
    je .Lbad_stack
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
.Lbad_stack:
    mov rsp, 0x800000000000
    jmp .Lforever
.Ltask:
    mov rax, [rsp + 16]
    cmp dword ptr [rax], 0x636f6c62
    je .Lblocking
    cmp dword ptr [rax], 0x6b736174
    jne .Lbad
    mov rax, [rsp + 24]
    movzx r12d, byte ptr [rax]
    sub r12d, 32
    cmp r12d, 65
    je .Ltask_setup
    cmp r12d, 66
    je .Ltask_setup
    cmp r12d, 70
    jne .Lbad
.Ltask_setup:
    lea rbx, [rip + zeroed]
    cmp qword ptr [rbx], 0
    jne .Lbad
    mov [rbx], r12
    mov ebp, 0x33
    mov ecx, 0x44
    mov edi, 0x55
    mov esi, 0x66
    mov edx, 0x77
    mov r8d, 0x88
    mov r9d, 0x99
    mov r10d, 0xaa
    mov r11d, 0xbb
    mov r13d, 0xcc
    mov r14d, 0xdd
    mov r15d, 0xee
    xor eax, eax
    sub rsp, 8
    mov qword ptr [rsp], 0x12345678
    std
.Ltask_loop:
    inc rax
    mov [rbx + 8], rax
    cmp qword ptr [rbx + 16], 0
    je .Ltask_loop
    cmp [rbx], r12
    jne .Lbad
    cmp [rbx + 8], rax
    jne .Lbad
    cmp rbp, 0x33
    jne .Lbad
    cmp rcx, 0x44
    jne .Lbad
    cmp rdi, 0x55
    jne .Lbad
    cmp rsi, 0x66
    jne .Lbad
    cmp rdx, 0x77
    jne .Lbad
    cmp r8, 0x88
    jne .Lbad
    cmp r9, 0x99
    jne .Lbad
    cmp r10, 0xaa
    jne .Lbad
    cmp r11, 0xbb
    jne .Lbad
    cmp r13, 0xcc
    jne .Lbad
    cmp r14, 0xdd
    jne .Lbad
    cmp r15, 0xee
    jne .Lbad
    cmp qword ptr [rsp], 0x12345678
    jne .Lbad
    pushfq
    pop rax
    and eax, 0x600
    cmp eax, 0x600
    jne .Lbad
    cmp r12d, 70
    je .Ltask_fault
    mov rdi, r12
    jmp .Lexit
.Ltask_fault:
    ud2
.Lblocking:
    mov rax, [rsp + 24]
    movzx r12d, byte ptr [rax]
    sub r12d, 32
    lea rbx, [rip + zeroed]
    mov [rbx], r12
    mov eax, 35
    xor edi, edi
    int 0x80
    test rax, rax
    jne .Lbad
    mov eax, 35
    mov rdi, -1
    int 0x80
    cmp rax, -22
    jne .Lbad
    cmp qword ptr [rbx + 48], 0
    je .Lblocking_setup
    xor eax, eax
    xor edi, edi
    lea rsi, [rbx + 64]
    mov edx, 3
    int 0x80
    cmp rax, 3
    jne .Lbad
    cmp word ptr [rbx + 64], 0x100
    jne .Lbad
    cmp byte ptr [rbx + 66], 2
    jne .Lbad
    mov qword ptr [rbx + 48], 2
.Lblocking_setup:
    mov ebp, 0x33
    mov ecx, 0x44
    mov esi, 0x66
    mov edx, 0x77
    mov r8d, 0x88
    mov r9d, 0x99
    mov r10d, 0xaa
    mov r11d, 0xbb
    xor r13d, r13d
    mov r14d, 0xdd
    mov r15d, 0xee
    sub rsp, 8
    mov qword ptr [rsp], 0x12345678
    std
.Lblocking_loop:
    inc r13
    mov [rbx + 8], r13
    mov edi, 0x55
    mov eax, 24
    int 0x80
    test rax, rax
    jne .Lbad
    cmp rdi, 0x55
    jne .Lbad
    rdtsc
    shl rdx, 32
    or rax, rdx
    mov [rbx + 32], rax
    mov edx, 0x77
    mov rdi, [rbx + 24]
    test rdi, rdi
    jnz .Lblocking_duration
    mov edi, 100000
.Lblocking_duration:
    mov eax, 35
    int 0x80
    test rax, rax
    jne .Lbad
    cmp rbp, 0x33
    jne .Lbad
    cmp rcx, 0x44
    jne .Lbad
    cmp rsi, 0x66
    jne .Lbad
    cmp rdx, 0x77
    jne .Lbad
    cmp r8, 0x88
    jne .Lbad
    cmp r9, 0x99
    jne .Lbad
    cmp r10, 0xaa
    jne .Lbad
    cmp r11, 0xbb
    jne .Lbad
    cmp r14, 0xdd
    jne .Lbad
    cmp r15, 0xee
    jne .Lbad
    cmp [rbx], r12
    jne .Lbad
    cmp [rbx + 8], r13
    jne .Lbad
    cmp qword ptr [rsp], 0x12345678
    jne .Lbad
    pushfq
    pop rax
    and eax, 0x600
    cmp eax, 0x600
    jne .Lbad
    rdtsc
    shl rdx, 32
    or rax, rdx
    sub rax, [rbx + 32]
    mov [rbx + 40], rax
    mov edx, 0x77
    cmp qword ptr [rbx + 16], 0
    je .Lblocking_loop
    mov rdi, r12
    jmp .Lexit
.Lsleep_forever:
    mov eax, 35
    mov edi, 1000000
    int 0x80
    test rax, rax
    jne .Lbad
    jmp .Lsleep_forever
.Lsleep_once:
    mov eax, 35
    mov edi, 10000
    int 0x80
    test rax, rax
    jne .Lbad
    mov edi, 71
    jmp .Lexit
.Lyield_forever:
    mov eax, 24
    int 0x80
    test rax, rax
    jne .Lbad
    jmp .Lyield_forever
.Lbad_blocking:
    mov rsp, 0x800000000000
    mov eax, 35
    mov edi, 1000
    int 0x80
    ud2
.Lblocking_gate:
    xor eax, eax
    int 0x80
    cmp rax, -38
    jne .Lbad
    mov eax, 24
    int 0x80
    cmp rax, -38
    jne .Lbad
    mov eax, 35
    mov edi, 1000
    int 0x80
    cmp rax, -38
    jne .Lbad
    mov edi, 70
    jmp .Lexit
.Linput:
    xor eax, eax
    mov edi, 1
    xor edx, edx
    int 0x80
    cmp rax, -9
    jne .Lbad
    xor eax, eax
    xor edi, edi
    mov rsi, 0x100000004ffe
    mov edx, 257
    int 0x80
    cmp rax, -22
    jne .Lbad
    xor eax, eax
    xor esi, esi
    mov edx, 1
    int 0x80
    cmp rax, -14
    jne .Lbad
    xor eax, eax
    lea rsi, [rip + _start]
    int 0x80
    cmp rax, -14
    jne .Lbad
    xor eax, eax
    mov rsi, 0x100000006fff
    mov byte ptr [rsi], 0x5a
    mov edx, 2
    int 0x80
    cmp rax, -14
    jne .Lbad
    cmp byte ptr [rsi], 0x5a
    jne .Lbad
    xor eax, eax
    mov rsi, -1
    xor edx, edx
    int 0x80
    test rax, rax
    jne .Lbad
    lea rbx, [rip + zeroed]
    xor r12d, r12d
    xor r13d, r13d
    mov ebp, 0x33
    mov ecx, 0x44
    mov r8d, 0x88
    mov r9d, 0x99
    mov r10d, 0xaa
    mov r11d, 0xbb
    mov r15d, 0xee
    sub rsp, 8
    mov qword ptr [rsp], 0x12345678
    std
.Linput_read:
    xor eax, eax
    xor edi, edi
    mov rsi, 0x100000004ffe
    mov edx, 256
    mov r14d, 0xdd
    int 0x80
    cmp rax, 256
    ja .Lbad
    cmp rbp, 0x33
    jne .Lbad
    cmp rcx, 0x44
    jne .Lbad
    test rdi, rdi
    jne .Lbad
    cmp r14, 0xdd
    jne .Lbad
    mov r14, rax
    mov rax, 0x100000004ffe
    cmp rsi, rax
    jne .Lbad
    cmp rdx, 256
    jne .Lbad
    cmp r8, 0x88
    jne .Lbad
    cmp r9, 0x99
    jne .Lbad
    cmp r10, 0xaa
    jne .Lbad
    cmp r11, 0xbb
    jne .Lbad
    cmp r15, 0xee
    jne .Lbad
    cmp qword ptr [rsp], 0x12345678
    jne .Lbad
    pushfq
    pop rax
    and eax, 0x600
    cmp eax, 0x600
    jne .Lbad
    test r14, r14
    je .Linput_eof
    xor eax, eax
.Linput_bytes:
    cmp [rsi + rax], r13b
    jne .Lbad
    inc r13b
    inc rax
    cmp rax, r14
    jne .Linput_bytes
    add r12, r14
    mov [rbx + 8], r12
    inc qword ptr [rbx + 24]
    jmp .Linput_read
.Linput_eof:
    mov edi, 72
    jmp .Lexit
.Lquota:
    lea r13, [rip + zeroed]
    xor r12d, r12d
.Lquota_wait:
    cmp [r13 + 16], r12
    je .Lquota_wait
    mov r12, [r13 + 16]
    mov eax, 1
    mov edi, 1
    mov rsi, -1
    xor edx, edx
    int 0x80
    test rax, rax
    jne .Lbad
    jmp .Lquota_wait
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

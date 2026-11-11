.section .rodata.kutox_user_tests,"a",@progbits
.equ USER_CODE, {code}
.equ USER_DATA, {data}
.equ USER_NEXT, {next}
.equ USER_STACK, {stack}

.macro BEGIN name
.global \name
\name:
.endm
.macro END name
.global \name\()_end
\name\()_end:
.endm
.macro EXIT status
    mov edi, \status
    mov eax, 60
    int 0x80
    ud2
.endm
.macro WRITE address, length
    mov eax, 1
    mov edi, 1
    mov rsi, \address
    mov edx, \length
    int 0x80
.endm

BEGIN kutox_guest_user_exit
    or rax, rcx
    or rax, rdx
    or rax, rbx
    or rax, rbp
    or rax, rsi
    or rax, rdi
    or rax, r8
    or rax, r9
    or rax, r10
    or rax, r11
    or rax, r12
    or rax, r13
    or rax, r14
    or rax, r15
    jnz .Lentry_bad
    mov ax, cs
    and eax, 3
    cmp eax, 3
    jne .Lentry_bad
    mov ax, ss
    and eax, 3
    cmp eax, 3
    jne .Lentry_bad
    pushfq
    pop rax
    test eax, 0x77500
    jnz .Lentry_bad
    test eax, 0x200
    jz .Lentry_bad
    EXIT 33
.Lentry_bad:
    EXIT 99
END kutox_guest_user_exit

BEGIN kutox_guest_user_syscalls
    mov eax, 0x7fff
    int 0x80
    cmp rax, -38
    jne .Lsys_bad
    mov eax, 1
    mov edi, 7
    mov rsi, USER_DATA
    mov edx, 1
    int 0x80
    cmp rax, -9
    jne .Lsys_bad
    mov rax, USER_DATA
    mov rsi, [rax]
    mov eax, 1
    mov edi, 1
    mov edx, 1
    int 0x80
    cmp rax, -14
    jne .Lsys_bad
    WRITE -1, 2
    cmp rax, -14
    jne .Lsys_bad
    WRITE USER_NEXT + 4096, 1
    cmp rax, -14
    jne .Lsys_bad
    WRITE USER_DATA, 257
    cmp rax, -22
    jne .Lsys_bad
    WRITE -1, 0
    test rax, rax
    jne .Lsys_bad
    WRITE USER_NEXT + 4094, 4
    cmp rax, -14
    jne .Lsys_bad
    mov rcx, 0x11223344
    mov r11, 0x55667788
    mov r12, 0x12345678
    std
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + .Lmessage]
    mov edx, OFFSET .Lmessage_length
    int 0x80
    cmp rax, OFFSET .Lmessage_length
    jne .Lsys_bad
    WRITE USER_DATA + 4094, 4
    cmp rax, 4
    jne .Lsys_bad
    cmp rcx, 0x11223344
    jne .Lsys_bad
    cmp r11, 0x55667788
    jne .Lsys_bad
    cmp r12, 0x12345678
    jne .Lsys_bad
    pushfq
    pop rax
    test eax, 0x400
    jz .Lsys_bad
    EXIT 0
.Lsys_bad:
    EXIT 99
.Lmessage:
    .ascii "# USER_WRITE hello\n"
.Lmessage_end:
.equ .Lmessage_length, .Lmessage_end - .Lmessage
END kutox_guest_user_syscalls

BEGIN kutox_guest_user_read
    mov rax, USER_DATA
    mov rax, [rax]
    mov rax, [rax]
    ud2
END kutox_guest_user_read

BEGIN kutox_guest_user_code_write
    mov rax, USER_CODE
    mov byte ptr [rax], 0
    ud2
END kutox_guest_user_code_write

BEGIN kutox_guest_user_nx
    mov rax, USER_STACK
    jmp rax
END kutox_guest_user_nx

BEGIN kutox_guest_user_cli
    cli
    ud2
END kutox_guest_user_cli

BEGIN kutox_guest_user_io
    mov edx, 0x80
    xor eax, eax
    out dx, al
    ud2
END kutox_guest_user_io

BEGIN kutox_guest_user_ud
    ud2
END kutox_guest_user_ud

BEGIN kutox_guest_user_fpu
    .byte 0xd9, 0xee
    ud2
END kutox_guest_user_fpu

BEGIN kutox_guest_user_limit
    mov eax, 0x7fff
    int 0x80
    jmp kutox_guest_user_limit
END kutox_guest_user_limit

BEGIN kutox_guest_user_bounds
    mov r12d, 4
.Lbound_loop:
    WRITE USER_DATA, 256
    cmp rax, 256
    jne .Lbound_bad
    dec r12d
    jnz .Lbound_loop
    WRITE USER_DATA, 256
    cmp rax, -22
    jne .Lbound_bad
    EXIT 0
.Lbound_bad:
    EXIT 99
END kutox_guest_user_bounds

BEGIN kutox_guest_user_bad_context
    mov rsp, 0x800000000000
    mov eax, 0x7fff
    int 0x80
    ud2
END kutox_guest_user_bad_context

BEGIN kutox_guest_user_nt
    push 0x4002
    popfq
    mov eax, 0x7fff
    int 0x80
    cmp rax, -38
    jne .Lnt_bad
    pushfq
    pop rax
    test eax, 0x4000
    jnz .Lnt_bad
    EXIT 33
.Lnt_bad:
    EXIT 99
END kutox_guest_user_nt

BEGIN kutox_guest_user_startup
    test rsp, 15
    jnz .Lstartup_bad
    cmp qword ptr [rsp], 2
    jne .Lstartup_bad
    mov rax, [rsp + 8]
    cmp dword ptr [rax], 0x626f7270
    jne .Lstartup_bad
    cmp word ptr [rax + 4], 0x65
    jne .Lstartup_bad
    mov rax, [rsp + 16]
    cmp dword ptr [rax], 0x00677261
    jne .Lstartup_bad
    cmp qword ptr [rsp + 24], 0
    jne .Lstartup_bad
    mov rax, [rsp + 32]
    cmp dword ptr [rax], 0x3d59454b
    jne .Lstartup_bad
    cmp dword ptr [rax + 4], 0x756c6176
    jne .Lstartup_bad
    cmp word ptr [rax + 8], 0x65
    jne .Lstartup_bad
    cmp qword ptr [rsp + 40], 0
    jne .Lstartup_bad
    cmp qword ptr [rsp + 48], 9
    jne .Lstartup_bad
    mov rax, USER_CODE
    cmp [rsp + 56], rax
    jne .Lstartup_bad
    cmp qword ptr [rsp + 64], 6
    jne .Lstartup_bad
    cmp qword ptr [rsp + 72], 4096
    jne .Lstartup_bad
    cmp qword ptr [rsp + 80], 0
    jne .Lstartup_bad
    cmp qword ptr [rsp + 88], 0
    jne .Lstartup_bad
    EXIT 42
.Lstartup_bad:
    EXIT 99
END kutox_guest_user_startup

BEGIN kutox_guest_user_startup_full
    mov rax, USER_STACK
    cmp rsp, rax
    jne .Lstartup_full_bad
    cmp qword ptr [rsp], 1
    jne .Lstartup_full_bad
    lea rax, [rsp + 64]
    cmp [rsp + 8], rax
    jne .Lstartup_full_bad
    cmp byte ptr [rax], 0x61
    jne .Lstartup_full_bad
    cmp byte ptr [rax + 4030], 0x61
    jne .Lstartup_full_bad
    cmp byte ptr [rax + 4031], 0
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 16], 0
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 24], 0
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 32], 6
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 40], 4096
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 48], 0
    jne .Lstartup_full_bad
    cmp qword ptr [rsp + 56], 0
    jne .Lstartup_full_bad
    EXIT 43
.Lstartup_full_bad:
    EXIT 99
END kutox_guest_user_startup_full

BEGIN kutox_guest_user_irq_wait
    mov eax, 0x10
    mov ecx, 0x11
    mov edx, 0x12
    mov ebx, 0x13
    mov ebp, 0x14
    mov rsi, USER_DATA
    mov edi, 0x16
    mov r8d, 0x18
    mov r9d, 0x19
    mov r10d, 0x1a
    mov r11d, 0x1b
    mov r12d, 0x1c
    mov r13d, 0x1d
    mov r14d, 0x1e
    mov r15d, 0x1f
    std
    mov byte ptr [rsi + 1], 1
.Lirq_wait:
    cmp byte ptr [rsi], 1
    jne .Lirq_wait
    cmp rax, 0x10
    jne .Lirq_bad
    cmp rcx, 0x11
    jne .Lirq_bad
    cmp rdx, 0x12
    jne .Lirq_bad
    cmp rbx, 0x13
    jne .Lirq_bad
    cmp rbp, 0x14
    jne .Lirq_bad
    cmp rdi, 0x16
    jne .Lirq_bad
    cmp r8, 0x18
    jne .Lirq_bad
    cmp r9, 0x19
    jne .Lirq_bad
    cmp r10, 0x1a
    jne .Lirq_bad
    cmp r11, 0x1b
    jne .Lirq_bad
    cmp r12, 0x1c
    jne .Lirq_bad
    cmp r13, 0x1d
    jne .Lirq_bad
    cmp r14, 0x1e
    jne .Lirq_bad
    cmp r15, 0x1f
    jne .Lirq_bad
    pushfq
    pop rax
    and eax, 0x600
    cmp eax, 0x600
    jne .Lirq_bad
    cld
    EXIT 45
.Lirq_bad:
    EXIT 99
END kutox_guest_user_irq_wait

BEGIN kutox_guest_user_irq_bad_stack
    mov rax, USER_DATA
    mov byte ptr [rax + 1], 1
    mov rsp, 0x800000000000
.Lirq_bad_stack_loop:
    jmp .Lirq_bad_stack_loop
END kutox_guest_user_irq_bad_stack

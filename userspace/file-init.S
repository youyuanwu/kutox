.intel_syntax noprefix
.section .text,"ax",@progbits
.global _start
_start:
    test rsp, 15
    jnz .Lbad
    cmp qword ptr [rsp], 1
    jne .Lbad
    sub rsp, 512
    mov rbp, rsp
    call .Lopen
    cmp rax, 3
    jne .Lbad
    mov r12, rax
#ifdef KUTOX_FILE_TIMEOUT
.Lspin:
    jmp .Lspin
#endif
    mov eax, 257
    mov rdi, -100
    xor esi, esi
    xor edx, edx
    int 0x80
    cmp rax, -14
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + unterminated]
    xor edx, edx
    int 0x80
    cmp rax, -36
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + path]
    mov edx, 32
    int 0x80
    cmp rax, -22
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + relative]
    xor edx, edx
    int 0x80
    cmp rax, -22
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + directory]
    xor edx, edx
    int 0x80
    cmp rax, -21
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + path]
    mov edx, 1
    int 0x80
    cmp rax, -30
    jne .Lbad
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + missing]
    xor edx, edx
    int 0x80
    cmp rax, -2
    jne .Lbad
    xor eax, eax
    mov edi, 99
    mov rsi, rbp
    mov edx, 1
    int 0x80
    cmp rax, -9
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    mov rsi, 0x10000000ffff
    mov edx, 2
    int 0x80
    cmp rax, -14
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    mov rsi, rbp
    mov edx, 257
    int 0x80
    cmp rax, -22
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    lea rsi, [rip + path]
    mov edx, 1
    int 0x80
    cmp rax, -14
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    mov rsi, rbp
    mov edx, 5
    int 0x80
    cmp rax, -5
    je .Lio
    cmp rax, 5
    jne .Lbad
    cmp dword ptr [rbp], 0x454c4946
    jne .Lbad
    cmp byte ptr [rbp + 4], ':'
    jne .Lbad
    call .Lopen
    cmp rax, 4
    jne .Lbad
    mov r13, rax
    xor eax, eax
    mov rdi, r13
    lea rsi, [rip + read_buffer + 4094]
    mov edx, 5
    int 0x80
    cmp rax, 5
    jne .Lbad
    cmp dword ptr [rsi], 0x454c4946
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    lea rsi, [rbp + 5]
    mov edx, 256
    int 0x80
    cmp rax, 28
    jne .Lbad
    mov eax, 1
    mov edi, 1
    mov rsi, rbp
    mov rdx, qword ptr [rip + message_length]
    int 0x80
    cmp rax, 33
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    mov rsi, rbp
    mov edx, 1
    int 0x80
    test rax, rax
    jne .Lbad
    xor eax, eax
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    int 0x80
    test rax, rax
    jne .Lbad
    mov eax, 3
    mov rdi, r13
    int 0x80
    test rax, rax
    jne .Lbad
    xor eax, eax
    mov rdi, r13
    mov rsi, rbp
    mov edx, 1
    int 0x80
    cmp rax, -9
    jne .Lbad
    call .Lopen
    cmp rax, 4
    jne .Lbad
    call .Lopen
    cmp rax, 5
    jne .Lbad
    call .Lopen
    cmp rax, 6
    jne .Lbad
    call .Lopen
    cmp rax, -24
    jne .Lbad
    mov eax, 3
    mov edi, 99
    int 0x80
    cmp rax, -9
    jne .Lbad
    lea rsi, [rip + success]
    lea rdx, [rip + success_end]
    sub rdx, rsi
    xor r14d, r14d
    jmp .Lwrite
.Lio:
    lea rsi, [rip + io]
    lea rdx, [rip + io_end]
    sub rdx, rsi
    xor r14d, r14d
    jmp .Lwrite
.Lbad:
    lea rsi, [rip + bad]
    lea rdx, [rip + bad_end]
    sub rdx, rsi
    mov r14d, 99
.Lwrite:
    mov eax, 1
    mov edi, 1
    int 0x80
    cmp rax, rdx
    jne .Lwrite_bad
    mov rdi, r14
    jmp .Lexit
.Lwrite_bad:
    mov edi, 99
.Lexit:
    mov eax, 60
    int 0x80
    ud2
.Lopen:
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + path]
    xor edx, edx
    int 0x80
    ret
path:
    .asciz "/etc/message"
missing:
    .asciz "/missing"
relative:
    .asciz "etc/message"
directory:
    .asciz "/etc"
unterminated:
    .fill 129, 1, 0x78
success:
    .ascii "FILE_API: checks passed\n"
success_end:
io:
    .ascii "FILE_API: EIO retained\n"
io_end:
bad:
    .ascii "FILE_API: checks failed\n"
bad_end:
.section .data,"aw",@progbits
message_length:
    .quad 33
.section .bss,"aw",@nobits
.balign 4096
read_buffer:
    .skip 8192

.intel_syntax noprefix
.section .text,"ax",@progbits
.global _start
_start:
    test rsp, 15
    jnz .Lbad
    cmp qword ptr [rsp], 1
    jne .Lbad
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    lea rdx, [rip + message_end]
    sub rdx, rsi
    int 0x80
    cmp rax, rdx
    jne .Lbad
    xor edi, edi
    jmp .Lexit
.Lbad:
    mov edi, 99
.Lexit:
    mov eax, 60
    int 0x80
    ud2
message:
    .ascii "INIT: hello from /bin/init\n"
message_end:

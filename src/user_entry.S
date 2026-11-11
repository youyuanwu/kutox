.section .text.kutox_user_entry,"ax",@progbits
.global kutox_user_enter
.type kutox_user_enter, @function
kutox_user_enter:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [r8], rsp
    push rcx
    push rsi
    push 0x202
    push rdx
    push rdi
    xor eax, eax
    xor ecx, ecx
    xor edx, edx
    xor ebx, ebx
    xor ebp, ebp
    xor esi, esi
    xor edi, edi
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    iretq
.size kutox_user_enter, .-kutox_user_enter

.global kutox_user_resume
.type kutox_user_resume, @function
kutox_user_resume:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
.size kutox_user_resume, .-kutox_user_resume

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
    mov [rcx], rsp
    mov rax, rdi
    push rdx
    push qword ptr [rax + 128]
    push qword ptr [rax + 136]
    push rsi
    push qword ptr [rax + 120]
    mov r15, [rax]
    mov r14, [rax + 8]
    mov r13, [rax + 16]
    mov r12, [rax + 24]
    mov r11, [rax + 32]
    mov r10, [rax + 40]
    mov r9, [rax + 48]
    mov r8, [rax + 56]
    mov rdi, [rax + 64]
    mov rsi, [rax + 72]
    mov rbp, [rax + 80]
    mov rbx, [rax + 88]
    mov rdx, [rax + 96]
    mov rcx, [rax + 104]
    mov rax, [rax + 112]
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

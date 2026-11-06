.section .text
.altmacro

.macro KUTOX_ISR number
.global kutox_isr_\number
.type kutox_isr_\number, @function
kutox_isr_\number:
    /* Normalize hardware error-code and no-error-code exception frames. */
    .if (\number != 8) && (\number != 10) && (\number != 11) && (\number != 12) && (\number != 13) && (\number != 14) && (\number != 17) && (\number != 21) && (\number != 29) && (\number != 30)
        push 0
    .endif
    push \number
    jmp kutox_exception_common
.size kutox_isr_\number, .-kutox_isr_\number
.endm

.set vector, 0
.rept 256
    KUTOX_ISR %vector
    .set vector, vector+1
.endr

.type kutox_exception_common, @function
kutox_exception_common:
    push rax
    push rcx
    push rdx
    push rbx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    cld
    mov rbx, rsp
    mov rdi, rsp
    and rsp, -16
    call kutox_exception_dispatch
    mov rsp, rbx
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    pop rdx
    pop rcx
    pop rax
    add rsp, 16
    iretq
.size kutox_exception_common, .-kutox_exception_common

.section .rodata
.p2align 3
.global kutox_exception_entries
kutox_exception_entries:
.macro KUTOX_POINTER number
    .quad kutox_isr_\number
.endm
.set vector, 0
.rept 256
    KUTOX_POINTER %vector
    .set vector, vector+1
.endr
.noaltmacro

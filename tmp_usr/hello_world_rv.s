.global _start

.text
_start:
    sd zero, -8(sp) # 测试内存写入（别忘了 *sp 是非法地址）
    sd a0, -16(sp)  # 测试内存读取
    li a7, 233
    la a0, msg
    li a1, 14
    ecall
    li a7, 93
    ecall

.data
msg: .ascii "Hello, World!\n"

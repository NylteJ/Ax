= 参考

#let item(head, content) = text(head, font: "KaiTi") + "：" + content

#enum(tight: false, spacing: 12.5pt)[
  #item(link("https://pubs.opengroup.org/onlinepubs/9799919799/mindex.html", [POSIX 标准 POSIX.1-2024]), [参考了其中规定的各类接口和行为。])
][
  #item([Linux 手册], [主要参考的是 #link("https://man7.org/linux/man-pages/", [man7.org]) 和 #link("https://linux.die.net/man/", [linux.die.net])，同样参考了许多接口、行为和概念等。])
][
  #item([Linux], [实现上，参考了 Linux 对 VFS 的各个层级、`struct Task` 指针的存放位置、信号跳板机制、KPTI 的处理方式。没有源码级的参考。])
][
  #item(link("https://doc.rust-lang.org/nightly/reference/", [Rust Reference]), [参考了 Rust 的各类语法、行为、规范等。])
][
  各 create 的 rust-doc，包括标准库和各个第三方库，参考了其中函数的用法与约束等。因数目繁杂，不在此一一列出。
][
  #item(link("https://rcore-os.cn/rCore-Tutorial-Book-v3/index.html", [rCore-Tutorial-Book 第三版]), [Ax 的早期版本就是跟着 rCore 教学一步步搭建起来的。])
][
  #item([RocketOS、NoAxiom、NighthawkOS、Del0n1x], [主要参考了一些设计思路。也有参考他们的文档编写。])
]

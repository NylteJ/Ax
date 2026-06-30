# Ax

Ax 是由北京理工大学 btw 队从零开始自主研发的宏内核操作系统，支持在 RISC-V 64 和 LoongArch 64 上运行。成员仅一人，其包揽了全部的工作。

截至 2026 年 6 月 30 日 23:11，Ax 在初赛排行榜中位列第 56 名。

更详细的信息，请见`初赛文档.pdf` 或 `docs`。

## 构建与运行方法

在此目录下运行 `make all` 生成双平台内核 ELF 文件与磁盘镜像。

其它可能有用的指令：

- `make rv`/`make la`：只生成某一版本的 ELF 和镜像。
- `make clean`：清理。
- `make autofix`：自动执行 `cargo autofix`。
- `make fmt`：格式化代码。
- `make run-rv`/`make run-la`：运行对应版本的内核。
- `make regen-disk`：重新生成磁盘镜像。
- `make debug-rv`/`make debug-la`：调试对应版本的内核。需要安装对应调试器，并在 WSL2 中运行。此时内核强制调试构建。

默认情况下，内核使用发布构建，可使用 `MODE=debug` 启用调试构建。

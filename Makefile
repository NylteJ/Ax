SHELL := /bin/bash

WHITELIST ?= .git .gitignore

MODE ?= release

ifeq ($(MODE), release)
    CARGOFLAGS = --release
	BUILDDICT = release
else ifeq ($(MODE), debug)
    CARGOFLAGS = --features "has-debug-info"
	BUILDDICT = debug
else
$(error "错误的 MODE: $(MODE). 应为 'release'(默认) 或 'debug'(debug target 强制使用).")
endif

RED    := \033[0;31m
GREEN  := \033[0;32m
NC     := \033[0m

.PHONY: all
all: clean disk_img/rv-init disk_img/la-init regen-disk rv la

.PHONY: clean
clean:
	@{ \
		echo "正在清理..."; \
		cd os; \
		cargo clean; \
		rm -f ../kernel-rv ../kernel-la; \
		rm -f ./src/linker_rv.ld ./src/linker_la.ld; \
		rm -f ./kernel-rv.debug ./kernel-la.debug; \
		rm -f ../disk.img ../disk-la.img; \
		rm -f ../disk_img/rv-init ../disk_img/la-init; \
		rm -f ../disk_img/lib/*; \
		rm -rf ./.cargo; \
		cd ../programs/init; \
		cargo clean; \
		printf "$(GREEN)清理完毕！$(NC)\n"; \
	}

.PHONY: fmt
fmt:
	@{\
		echo "正在格式化代码..."; \
		cd os; \
		cargo fmt; \
		cd ../programs/init; \
		cargo fmt; \
		printf "$(GREEN)代码格式化完成！$(NC)\n"; \
	}

.PHONY: rv
rv: os/.cargo
	@{ \
		cd os; \
		printf "正在写入 linker_rv.ld ..."; \
		cat ./src/linker_rv.ld.template ./src/linker_body.ld.template > ./src/linker_rv.ld && echo "完成。" || { printf "$(RED)失败！$(NC)\n"; exit 1; }; \
		echo "正在编译 RISC-V 版本的内核..."; \
		cargo build --target riscv64gc-unknown-none-elf $(CARGOFLAGS) && printf "$(GREEN)RISC-V 版本的内核编译完成！$(NC)\n" || { printf "$(RED)RISC-V 版本的内核编译失败......$(NC)\n"; exit 1; }; \
		cp target/riscv64gc-unknown-none-elf/$(BUILDDICT)/os ../kernel-rv; \
		printf "$(GREEN)RISC-V 版本的 ELF 文件已输出到 kernel-rv！$(NC)\n"; \
		if [ "$(MODE)" = "debug" ]; then \
			printf "检测到调试版本，正在嵌入调试信息...\n"; \
			rust-objcopy --only-keep-debug ../kernel-rv ./kernel-rv.debug; \
			rust-objcopy --update-section .debug_infos=./kernel-rv.debug ../kernel-rv ../kernel-rv; \
			printf "$(GREEN)调试信息嵌入完毕，最终文件已输出到 kernel-rv！$(NC)\n"; \
		fi; \
	}

.PHONY: la
la: os/.cargo
	@{ \
		cd os; \
		printf "正在写入 linker_la.ld ..."; \
		cat ./src/linker_la.ld.template ./src/linker_body.ld.template > ./src/linker_la.ld && echo "完成。" || { printf "$(RED)失败！$(NC)\n"; exit 1; }; \
		echo "正在编译龙芯版本的内核..."; \
		cargo build --target loongarch64-unknown-none $(CARGOFLAGS) && printf "$(GREEN)龙芯版本的内核编译完成！$(NC)\n" || { printf "$(RED)龙芯版本的内核编译失败......$(NC)\n"; exit 1; }; \
		cp target/loongarch64-unknown-none/$(BUILDDICT)/os ../kernel-la; \
		printf "$(GREEN)龙芯版本的 ELF 文件已输出到 kernel-la！$(NC)\n"; \
		if [ "$(MODE)" = "debug" ]; then \
			printf "检测到调试版本，正在嵌入调试信息...\n"; \
			rust-objcopy --only-keep-debug ../kernel-la ./kernel-la.debug; \
			rust-objcopy --update-section .debug_infos=./kernel-la.debug ../kernel-la ../kernel-la; \
			printf "$(GREEN)调试信息嵌入完毕，最终文件已输出到 kernel-la！$(NC)\n"; \
		fi; \
	}

os/.cargo:
	cp os/cargo os/.cargo -r

disk_img/rv-init: $(wildcard programs/init/src/*)
	@{ \
		cd programs/init; \
		echo "正在编译 init..."; \
		cargo build --release --target riscv64gc-unknown-none-elf && printf "$(GREEN)init 编译完成！$(NC)\n" || { printf "$(RED)init 编译失败......$(NC)\n"; exit 1; }; \
		cp target/riscv64gc-unknown-none-elf/release/init ../../disk_img/rv-init; \
	}
disk_img/la-init: $(wildcard programs/init/src/*)
	@{ \
		cd programs/init; \
		echo "正在编译 init..."; \
		cargo build --release --target loongarch64-unknown-none && printf "$(GREEN)init 编译完成！$(NC)\n" || { printf "$(RED)init 编译失败......$(NC)\n"; exit 1; }; \
		cp target/loongarch64-unknown-none/release/init ../../disk_img/la-init; \
	}

.PHONY: check
check: clean
	@{ \
		GITIGNORE_LIST=$$( [ -f .gitignore ] && grep -v '^#' .gitignore | grep -v '^$$' | sed 's|^[[:space:]]*||;s|[[:space:]]*$$||;s|/$$||' || true ); \
		USER_LIST=$$(printf '%s\n' $(WHITELIST) .whitelist.tmp | sed 's|^[[:space:]]*||;s|[[:space:]]*$$||;s|/$$||'); \
		FULL_WHITELIST=$$(printf '%s\n%s' "$$USER_LIST" "$$GITIGNORE_LIST" | sort -u | sed '/^$$/d'); \
		printf "$$FULL_WHITELIST\n" > .whitelist.tmp; \
		violations=0; \
		while IFS= read -r -d '' item; do \
			rel_path=$${item#./}; \
			if ! grep -Fxq "$$rel_path" .whitelist.tmp && ! grep -Fxq "$$(basename "$$rel_path")" .whitelist.tmp; then \
				printf "$(RED)发现隐藏文件（夹）：$$item$(NC)\n"; \
				violations=$$((violations + 1)); \
			fi; \
		done < <(find . -name ".*" ! -name "." ! -name ".." -print0); \
		rm -f .whitelist.tmp; \
		if [ $$violations -eq 0 ]; then \
			printf "$(GREEN)未找到隐藏文件！$(NC)\n"; \
			exit 0; \
		else \
			printf "$(RED)共计发现了 $$violations 个隐藏文件（夹）$(NC)\n"; \
			exit 1; \
		fi; \
	}

MEMORY ?= 1024M
SMP ?= 1

RV_QEMU_CMD_BASE := \
	qemu-system-riscv64 \
		-machine virt \
		-kernel /repo/kernel-rv \
		-m $(MEMORY) \
		-nographic \
		-smp $(SMP) \
		-bios default \
		-no-reboot \
		-device virtio-net-device,netdev=net \
		-netdev user,id=net \
		-rtc base=utc \
		-drive file=/repo/disk.img,if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-drive file=/repo/sdcard-rv-pub.img,if=none,format=raw,id=x1 \
		-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1 \

LA_QEMU_CMD_BASE := \
	qemu-system-loongarch64 \
		-kernel /repo/kernel-la \
		-m $(MEMORY) \
		-nographic \
		-smp $(SMP) \
		-drive file=/repo/disk-la.img,if=none,format=raw,id=x1 \
 		-device virtio-blk-pci,drive=x1 \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0 \
		-rtc base=utc \
		-drive file=/repo/sdcard-la-pub.img,if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 \
# 		-device virtio-blk-pci,drive=x0,bus=virtio-mmio-bus.0 \
# 		-device virtio-blk-pci,drive=x1,bus=virtio-mmio-bus.1 \

WINDOWS_TERMINAL_PATH ?= /mnt/d/Downloads/Chrome/terminal-1.22.11751.0/wt.exe

# DOCKER_ARGS_BASE := -it --rm -v .:/repo -p 1234:1234 zhouzhouyi/os-contest:20260104
DOCKER_ARGS_BASE := --rm -v .:/repo -p 1234:1234 zhouzhouyi/os-contest:20260104

.PHONY: debug-rv
debug-rv: disk.img
	@{ \
		$(MAKE) rv MODE=debug || exit 1; \
		$(WINDOWS_TERMINAL_PATH) --profile "Ubuntu" -- bash -c "cd ~/OSKernel2026/repo && docker run $(DOCKER_ARGS_BASE) $(RV_QEMU_CMD_BASE) -s -S"; \
		sleep 2; \
		gdb-multiarch -ex "file ./kernel-rv" -ex "set arch riscv:rv64" -ex "target remote localhost:1234" -ex "tb *0x80200000" -ex "b os::basic::debugger::debugbreak" -ex "c" -ex "tui enable" -ex "layout split"; \
		docker stop $$(docker ps -q); \
		echo "调试完成，所有容器已终止"; \
	}
.PHONY: debug-la
debug-la: disk-la.img
	@{ \
		$(MAKE) la MODE=debug || exit 1; \
		$(WINDOWS_TERMINAL_PATH) --profile "Ubuntu" -- bash -c "cd ~/OSKernel2026/repo && docker run $(DOCKER_ARGS_BASE) $(LA_QEMU_CMD_BASE) -s -S"; \
		sleep 2; \
		~/OSKernel2026/cross-tools/bin/loongarch64-unknown-linux-gnu-gdb -ex "file ./kernel-la" -ex "target remote localhost:1234" -ex "b os::basic::debugger::debugbreak"; \
		docker stop $$(docker ps -q); \
		echo "调试完成，所有容器已终止"; \
	}

.PHONY: run-rv
run-rv: rv disk.img
	@{ \
		docker run $(DOCKER_ARGS_BASE) $(RV_QEMU_CMD_BASE); \
	}
.PHONY: run-la
run-la: la disk-la.img
	@{ \
		docker run $(DOCKER_ARGS_BASE) $(LA_QEMU_CMD_BASE); \
	}

.PHONY: clippy-rv
clippy-rv:
	@{ \
		cd os; \
		cargo clippy --target riscv64gc-unknown-none-elf $(CARGOFLAGS); \
	}
.PHONY: clippy-la
clippy-la:
	@{ \
		cd os; \
		cargo clippy --target loongarch64-unknown-none $(CARGOFLAGS); \
	}

disk.img: $(wildcard disk_img/*) disk_img/rv-init disk_img/la-init
	mkfs.ext4 -F -d ./disk_img disk.img 4M

disk-la.img: disk.img
	cp disk.img disk-la.img

.PHONY: regen-disk
regen-disk:
	rm -f disk.img disk-la.img
	$(MAKE) disk.img disk-la.img

.PHONY: mount-disk
mount-disk: disk.img
	mount -o ro,loop disk.img ./disk_img

.PHONY: umount-disk
umount-disk:
	umount ./disk_img

.PHONY: autofix
autofix:
	@{ \
		cd os; \
		cargo fix --bin "os" --allow-staged --target riscv64gc-unknown-none-elf $(CARGOFLAGS); \
	}

.PHONY: test
test: all check
# TODO

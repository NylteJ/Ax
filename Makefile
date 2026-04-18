SHELL := /bin/bash

WHITELIST ?= .git .gitignore

MODE ?= release

ifeq ($(MODE), release)
    CARGOFLAGS = --release
	BUILDDICT = release
else ifeq ($(MODE), debug)
    CARGOFLAGS =
	BUILDDICT = debug
else
$(error "错误的 MODE: $(MODE). 应为 'release'(默认) 或 'debug'(debug target 强制使用).")
endif

RED    := \033[0;31m
GREEN  := \033[0;32m
NC     := \033[0m

.PHONY: all
all: rv la

.PHONY: clean
clean:
	@{ \
		cd os; \
		echo "正在清理..."; \
		cargo clean; \
		rm -f ../kernel-rv ../kernel-la; \
		rm -f ./src/linker_rv.ld ./src/linker_la.ld; \
		rm -rf ./.cargo; \
		printf "$(GREEN)清理完毕！$(NC)\n"; \
	}

.PHONY: fmt
fmt:
	@{\
		cd os; \
		echo "正在格式化代码..."; \
		cargo fmt; \
		printf "$(GREEN)代码格式化完成！$(NC)\n"; \
	}

.PHONY: rv
rv: clean os/.cargo
	@{ \
		cd os; \
		printf "正在写入 linker_rv.ld ..."; \
		cat ./src/linker_rv.ld.template ./src/linker_body.ld.template > ./src/linker_rv.ld && echo "完成。" || { printf "$(RED)失败！$(NC)\n"; exit 1; }; \
		echo "正在编译 RISC-V 版本的内核..."; \
		cargo build --target riscv64gc-unknown-none-elf $(CARGOFLAGS) && printf "$(GREEN)RISC-V 版本的内核编译完成！$(NC)\n" || { printf "$(RED)RISC-V 版本的内核编译失败......$(NC)\n"; exit 1; }; \
		cp target/riscv64gc-unknown-none-elf/$(BUILDDICT)/os ../kernel-rv; \
		printf "$(GREEN)RISC-V 版本的 ELF 文件已输出到 kernel-rv！$(NC)\n"; \
	}

.PHONY: la
la: clean os/.cargo
	@{ \
		cd os; \
		printf "正在写入 linker_la.ld ..."; \
		cat ./src/linker_la.ld.template ./src/linker_body.ld.template > ./src/linker_la.ld && echo "完成。" || { printf "$(RED)失败！$(NC)\n"; exit 1; }; \
		echo "正在编译龙芯版本的内核..."; \
		cargo build --target loongarch64-unknown-none $(CARGOFLAGS) && printf "$(GREEN)龙芯版本的内核编译完成！$(NC)\n" || { printf "$(RED)龙芯版本的内核编译失败......$(NC)\n"; exit 1; }; \
		cp target/loongarch64-unknown-none/$(BUILDDICT)/os ../kernel-la; \
		printf "$(GREEN)龙芯版本的 ELF 文件已输出到 kernel-la！$(NC)\n"; \
	}

os/.cargo:
	cp os/cargo os/.cargo -r

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
# 		-drive file=disk.img,if=none,format=raw,id=x1 \
# 		-device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1 \
# 		-drive file={fs},if=none,format=raw,id=x0 \
# 		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \

LA_QEMU_CMD_BASE := \
	qemu-system-loongarch64 \
		-kernel /repo/kernel-la \
		-m $(MEMORY) \
		-nographic \
		-smp $(SMP) \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0,hostfwd=tcp::5555-:5555,hostfwd=udp::5555-:5555 \
		-rtc base=utc \
# 		-drive file=disk-la.img,if=none,format=raw,id=x1 \
# 		-device virtio-blk-pci,drive=x1,bus=virtio-mmio-bus.1 \
# 		-drive file={fs},if=none,format=raw,id=x0 \
# 		-device virtio-blk-pci,drive=x0,bus=virtio-mmio-bus.0 \

DOCKER_ARGS_BASE := --rm -v .:/repo -p 1234:1234 zhouzhouyi/os-contest:20260104

.PHONY: debug-rv
debug-rv:
	@{ \
		$(MAKE) rv MODE=debug || exit 1; \
		docker run -d $(DOCKER_ARGS_BASE) $(RV_QEMU_CMD_BASE) -s -S; \
		gdb-multiarch -ex "file ./kernel-rv" -ex "set arch riscv:rv64" -ex "target remote localhost:1234" -ex "tb *0x80200000" -ex "c" -ex "tui enable" -ex "layout split"; \
		docker stop $$(docker ps -q); \
		echo "调试完成，所有容器已终止"; \
	}
.PHONY: debug-la
debug-la:
	@{ \
		$(MAKE) la MODE=debug || exit 1; \
		docker run -d $(DOCKER_ARGS_BASE) $(LA_QEMU_CMD_BASE) -s -S; \
		sleep 0.1; \
		~/OSKernel2026/cross-tools/bin/loongarch64-unknown-linux-gnu-gdb -ex "file ./kernel-la" -ex "target remote localhost:1234"; \
		docker stop $$(docker ps -q); \
		echo "调试完成，所有容器已终止"; \
	}

.PHONY: run-rv
run-rv: rv
	@{ \
		docker run -it $(DOCKER_ARGS_BASE) $(RV_QEMU_CMD_BASE); \
	}
.PHONY: run-la
run-la: la
	@{ \
		docker run -it $(DOCKER_ARGS_BASE) $(LA_QEMU_CMD_BASE); \
	}

.PHONY: test
test: all check
# TODO

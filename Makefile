# CantayaOS — Makefile
#
# Targets:
#   make build     — compile bootloader (UEFI) + kernel (AArch64 ELF)
#   make run       — launch QEMU with OVMF + built images
#   make smoke     — boot QEMU headlessly and validate the runtime markers
#   make clean     — remove build artefacts
#   make iso       — build a FAT32 boot image (requires mtools)
#
# Requirements on macOS:
#   brew install qemu mtools
#   Download OVMF_CODE.fd for AArch64 — see README.md

ARCH       := aarch64
TARGET_BOOT := aarch64-unknown-uefi
TARGET_KERN := $(CURDIR)/kernel/aarch64-cantaya.json
TARGET_USER := $(CURDIR)/user/aarch64-cantaya-user.json

CARGO      := cargo
BUILD_DIR  := target

# Bootloader output (PE32+ EFI application)
BOOT_EFI   := $(BUILD_DIR)/$(TARGET_BOOT)/release/cantaya-boot.efi

KERN_ELF   := $(BUILD_DIR)/aarch64-cantaya/release/cantaya-kernel
USER_ELF   := $(BUILD_DIR)/aarch64-cantaya-user/release/cantaya-user-init
USER_CHILD_ELF := $(BUILD_DIR)/aarch64-cantaya-user/release/cantaya-user-child

# OVMF firmware (download separately — see README)
OVMF       := $(CURDIR)/tools/OVMF_CODE.fd
OVMF_VARS  := $(CURDIR)/tools/edk2-arm-vars.fd

# Disk image
DISK_IMG   := $(BUILD_DIR)/cantaya.img
DISK_IMG_NEW := $(DISK_IMG).new
ESP_DIR    := $(BUILD_DIR)/esp

QEMU       := qemu-system-aarch64
QEMU_FLAGS := \
  -name CantayaOS \
  -machine virt,highmem=on \
  -cpu cortex-a57 \
  -m 512M \
  -device ramfb \
  -device virtio-keyboard-device \
  -nic none \
  -drive if=pflash,format=raw,file=$(OVMF),readonly=on \
  -drive if=pflash,format=raw,file=$(OVMF_VARS) \
	-drive if=none,format=raw,file=$(DISK_IMG),id=cantaya-disk \
	-global virtio-mmio.force-legacy=false \
	-device virtio-blk-device,drive=cantaya-disk \
  -serial stdio \
  -display cocoa \
  -no-reboot

.PHONY: build user run smoke clean iso firmware-check

# ─────────────────────────────────────────────────────────────────────────────
user:
	@echo "==> Building init user process (aarch64-cantaya-user)"
	$(CARGO) build --release \
	    -p cantaya-user-init \
	    --target $(TARGET_USER) \
	    -Z json-target-spec \
	    -Z build-std=core,compiler_builtins \
	    -Z build-std-features=compiler-builtins-mem
	@echo "==> Building child user process (aarch64-cantaya-user)"
	$(CARGO) build --release \
	    -p cantaya-user-child \
	    --target $(TARGET_USER) \
	    -Z json-target-spec \
	    -Z build-std=core,compiler_builtins \
	    -Z build-std-features=compiler-builtins-mem

# ─────────────────────────────────────────────────────────────────────────────
build: firmware-check user
	@echo "==> Building bootloader ($(TARGET_BOOT))"
	$(CARGO) build --release \
	    -p cantaya-boot \
	    --target $(TARGET_BOOT)
	@echo "==> Building kernel (aarch64-cantaya)"
	cargo build --release \
	    -p cantaya-kernel \
	    --target $(TARGET_KERN) \
	    -Z json-target-spec \
	    -Z build-std=core,compiler_builtins,alloc \
	    -Z build-std-features=compiler-builtins-mem

# ─────────────────────────────────────────────────────────────────────────────
iso: build
	@echo "==> Creating ESP disk image"
	mkdir -p $(ESP_DIR)/EFI/BOOT
	mkdir -p $(ESP_DIR)/EFI/CantayaOS
	cp $(BOOT_EFI)  $(ESP_DIR)/EFI/BOOT/BOOTAA64.EFI
	cp $(KERN_ELF)  $(ESP_DIR)/EFI/CantayaOS/kernel.elf
	cp $(USER_ELF)  $(ESP_DIR)/EFI/CantayaOS/init.elf
	# EFI shell startup script — auto-launches bootloader on shell fallback
	printf 'FS0:\\EFI\\BOOT\\BOOTAA64.EFI\r\n' > $(ESP_DIR)/startup.nsh
	# Create a 128 MiB FAT32 disk image
	dd if=/dev/zero of=$(DISK_IMG_NEW) bs=1M count=128 2>/dev/null
	mformat -i $(DISK_IMG_NEW) -F ::
	mcopy -s -i $(DISK_IMG_NEW) $(ESP_DIR)/EFI ::/EFI
	mcopy    -i $(DISK_IMG_NEW) $(USER_CHILD_ELF) ::/CHILD.ELF
	mcopy    -i $(DISK_IMG_NEW) $(ESP_DIR)/startup.nsh ::/startup.nsh
	mv $(DISK_IMG_NEW) $(DISK_IMG)
	@echo "==> Disk image: $(DISK_IMG)"

# ─────────────────────────────────────────────────────────────────────────────
run: iso
	@echo "==> Launching QEMU (AArch64 virt + OVMF)"
	$(QEMU) $(QEMU_FLAGS)

# ─────────────────────────────────────────────────────────────────────────────
smoke: iso
	@python3 scripts/qemu_smoke.py \
	    --qemu "$(QEMU)" \
	    --ovmf "$(OVMF)" \
	    --ovmf-vars "$(OVMF_VARS)" \
	    --image "$(DISK_IMG)"

# ─────────────────────────────────────────────────────────────────────────────
clean:
	$(CARGO) clean
	rm -rf $(ESP_DIR) $(DISK_IMG) $(DISK_IMG_NEW)

# ─────────────────────────────────────────────────────────────────────────────
firmware-check:
	@if [ ! -f $(OVMF) ]; then \
	    echo ""; \
	    echo "ERROR: OVMF firmware not found at $(OVMF)"; \
	    echo ""; \
	    echo "Download it with:"; \
	    echo "  mkdir -p tools"; \
	    echo "  # Option A — from Fedora RPM (cross-platform):"; \
	    echo "  curl -L https://dl.fedoraproject.org/pub/fedora/linux/releases/40/Everything/aarch64/os/Packages/e/edk2-aarch64-20240524-3.fc40.noarch.rpm -o /tmp/edk2.rpm"; \
	    echo "  cd /tmp && rpm2cpio edk2.rpm | cpio -idmv"; \
	    echo "  cp /tmp/usr/share/edk2/aarch64/QEMU_EFI.fd  $(OVMF)"; \
	    echo "  cp /tmp/usr/share/edk2/aarch64/QEMU_VARS.fd $(OVMF_VARS)"; \
	    echo "  # Option B — brew (if available):"; \
	    echo "  brew install edk2"; \
	    echo ""; \
	    exit 1; \
	fi

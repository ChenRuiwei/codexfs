export RUST_BACKTRACE = 1
export RUST_LOG = info

SOURCE := simple.tmp
MNT := mnt/
IMAGE := simple.img
MODE := debug

MKFS_ARGS :=
MKFS_ARGS += $(IMAGE)
MKFS_ARGS += $(SOURCE)

FUSE_ARGS :=
FUSE_ARGS += $(IMAGE)
FUSE_ARGS += $(MNT)

CARGO_ARGS :=
ifeq ($(MODE), release)
	 CARGO_ARGS += --release
endif

.PHONY: mkfs
mkfs:
	cargo run $(CARGO_ARGS) --package codexfs-mkfs -- $(MKFS_ARGS)

.PHONY: mkfs-gdb
mkfs-gdb:
	rust-gdb --args target/$(MODE)/codexfs-mkfs $(MKFS_ARGS)

.PHONY: fuse
fuse:
	cargo run $(CARGO_ARGS) --package codexfs-fuse -- $(FUSE_ARGS)

.PHONY: fuse-gdb
fuse-gdb:
	rust-gdb --args target/$(MODE)/codexfs-fuse -- $(FUSE_ARGS)

.PHONY: tokei
tokei:
	tokei -e crates

.PHONY: test
test:
	cargo test

.PHONY: clean
clean:
	cargo clean
	rm -rf *.img

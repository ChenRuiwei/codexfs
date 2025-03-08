export RUST_BACKTRACE = 1
export RUST_LOG = info

SOURCE := simple.tmp
MNT := mnt/
IMAGE := simple.img
MODE := debug

CARGO_ARGS :=
ifeq ($(MODE), release)
	 CARGO_ARGS += --release
endif

.PHONY: mkfs
mkfs:
	cargo run $(CARGO_ARGS) --package codexfs-mkfs -- $(IMAGE) $(SOURCE)

.PHONY: mkfs-gdb
mkfs-gdb:
	rust-gdb --args target/$(MODE)/codexfs-mkfs $(IMAGE) $(SOURCE)

.PHONY: fuse
fuse:
	cargo run $(CARGO_ARGS) --package codexfs-fuse -- $(IMAGE) $(MNT)

.PHONY: fuse-gdb
fuse-gdb:
	rust-gdb --args target/$(MODE)/codexfs-fuse -- $(IMAGE) $(MNT)

.PHONY: test
test:
	cargo test

export RUST_BACKTRACE = 1
export RUST_LOG = info

.PHONY: mkfs
mkfs:
	cargo run --package codexfs-mkfs -- img.tmp tmp

.PHONY: mkfs-gdb
mkfs-gdb:
	rust-gdb --args target/debug/codexfs-mkfs img.tmp tmp

.PHONY: fuse
fuse:
	cargo run --package codexfs-fuse -- img.tmp mnt

.PHONY: fuse-gdb
fuse-gdb:
	rust-gdb --args target/debug/codexfs-fuse -- img.tmp mnt

.PHONY: test
test:
	cargo test

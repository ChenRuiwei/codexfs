export RUST_BACKTRACE = 1

.PHONY: mkfs
mkfs:
	cargo run --package codexfs-mkfs -- img.tmp tmp

.PHONY: fuse
fuse:
	cargo run --package codexfs-fuse -- img.tmp mnt

.PHONY: test
test:
	cargo test

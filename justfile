export RUST_BACKTRACE := "1"
export RUST_LOG := "info"

SOURCE := "simple.tmp"
MNT := "mnt"
IMAGE := "simple.img"
MODE := "debug"

MKFS_ARGS := IMAGE + " " + SOURCE
FUSE_ARGS := IMAGE + " " + MNT

CARGO_ARGS := if MODE == "release" {
        " --release"
    } else {
        ""
    }

default:
    @just --list

mkfs *MKFS_EXTRA_ARGS:
	cargo run {{CARGO_ARGS}} --package codexfs-mkfs -- {{MKFS_ARGS}} {{MKFS_EXTRA_ARGS}}

mkfs-gdb *MKFS_EXTRA_ARGS:
	rust-gdb --args target/{{MODE}}/codexfs-mkfs {{MKFS_ARGS}} {{MKFS_EXTRA_ARGS}}

fuse:
	cargo run {{CARGO_ARGS}} --package codexfs-fuse -- {{FUSE_ARGS}}

fuse-gdb:
	rust-gdb --args target/{{MODE}}/codexfs-fuse -- {{FUSE_ARGS}}

tokei:
	tokei -e crates

test:
	cargo test

clean:
	cargo clean
	rm -rf *.img

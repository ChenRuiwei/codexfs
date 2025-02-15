// FIXME: types should be aligned when dumping
// FIXME: dirent offset is not calculated yet
#![allow(static_mut_refs)]

use std::{cell::OnceCell, fs::File, path::Path};

use clap::Parser;
use codexfs_core::{
    inode,
    sb::{self, get_mut_sb, get_sb, set_sb},
    utils::round_up,
    CODEXFS_BLKSIZ,
};

#[derive(Debug, Parser)]
#[command(name = "mkfs.codexfs")]
#[command(version("1.0"))]
#[command(about = "A command-line tool to create an CODEX filesystem")]
struct Args {
    #[arg(index(1))]
    pub img_path: String,
    #[arg(index(2))]
    pub src_path: String,
}

static mut ARGS: OnceCell<Args> = OnceCell::new();

fn get_args() -> &'static Args {
    unsafe { ARGS.get().unwrap() }
}

fn set_args(args: Args) {
    unsafe {
        ARGS.set(args).unwrap();
    }
}

fn parse_args() -> &'static Args {
    let args = Args::parse();
    set_args(args);
    get_args()
}

fn main() {
    let args = parse_args();
    let img_file = File::create(&args.img_path).unwrap();
    set_sb(img_file);
    let root = inode::mkfs_load_inode(Path::new(&args.src_path), None).unwrap();
    get_mut_sb().set_root(root);
    let root = get_mut_sb().get_root();

    inode::mkfs_calc_inode_off(root);

    get_mut_sb().set_start_off(round_up(get_sb().get_start_off(), CODEXFS_BLKSIZ as _));
    inode::mkfs_dump_inode_tree(root).unwrap();
    root.borrow().print_recursive(0);
    sb::mkfs_dump_super_block().unwrap();
}

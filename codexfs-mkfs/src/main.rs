// FIXME: types should be aligned when dumping
// FIXME: dirent offset is not calculated yet
#![allow(static_mut_refs)]

use std::{
    cell::{OnceCell, RefCell},
    path::Path,
    rc::Rc,
};

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
    set_sb(Path::new(&args.img_path));
    let root = inode::load_inode_tree(Path::new(&args.src_path)).unwrap();
    get_mut_sb().init_root(Rc::new(RefCell::new(root)));
    let root = get_mut_sb().get_root();

    inode::calc_inode_off(root);

    get_mut_sb().set_start_off(round_up(get_sb().get_start_off(), CODEXFS_BLKSIZ as _));
    inode::dump_inode_tree(root).unwrap();
    root.borrow().print_recursive(0);
    sb::dump_super_block().unwrap();
}

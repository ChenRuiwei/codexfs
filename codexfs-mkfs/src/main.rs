#![allow(static_mut_refs)]

use std::{cell::OnceCell, fs::File, path::Path};

use clap::Parser;
use codexfs_core::{
    blk_size_t,
    compress::{get_cmpr_mgr_mut, set_cmpr_mgr},
    inode,
    sb::{self, SuperBlock, get_sb, get_sb_mut, set_sb},
};

#[derive(Debug, Parser)]
#[command(name = "mkfs.codexfs")]
#[command(version("1.0"))]
#[command(about = "A command-line tool to create an CODEX filesystem")]
struct Args {
    #[arg(short, long, action)]
    pub uncompress: bool,
    #[arg(short, long, default_value_t = 4096)]
    pub blksz: blk_size_t,
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
    env_logger::init();

    let args = parse_args();
    let img_file = File::create(&args.img_path).unwrap();
    set_sb(SuperBlock::new(img_file, args.blksz.ilog2() as _));
    get_sb_mut().compress = !args.uncompress;
    assert_eq!(get_sb().blksz(), args.blksz, "invalid blksz");
    set_cmpr_mgr(6);
    let root = inode::mkfs_load_inode(Path::new(&args.src_path), None).unwrap();
    get_sb_mut().set_root(root);

    sb::mkfs_balloc_super_block();
    inode::get_inode_vec_mut()
        .iter()
        .for_each(|i| println!("{:?}", i.meta().path));

    if get_sb().compress {
        get_cmpr_mgr_mut().reorder();
        inode::mkfs_dump_inode_file_data_z().unwrap();
    } else {
        inode::mkfs_dump_inode_file_data().unwrap();
    }
    inode::mkfs_balloc_inode();
    inode::mkfs_dump_inode().unwrap();
    sb::mkfs_dump_super_block().unwrap();
    sb::mkfs_align_block_size().unwrap();
}

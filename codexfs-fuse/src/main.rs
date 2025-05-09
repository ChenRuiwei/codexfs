#![feature(once_cell_get_mut)]
#![allow(static_mut_refs)]

mod fuse;

use std::{cell::OnceCell, fs::File};

use clap::Parser;
use codexfs_core::sb;
use fuse::CodexFs;
use fuser::MountOption;

#[derive(Debug, Parser)]
#[command(name = "codexfsfuse")]
#[command(version("1.0"))]
struct Args {
    #[arg(index(1))]
    pub img_path: String,
    #[arg(index(2))]
    pub mnt_path: String,
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
    let img_file = File::open(&args.img_path).unwrap();
    sb::fuse_load_super_block(img_file).unwrap();

    let options = vec![MountOption::FSName("fuser".to_string())];
    fuser::mount2(CodexFs, &args.mnt_path, &options).unwrap();
}

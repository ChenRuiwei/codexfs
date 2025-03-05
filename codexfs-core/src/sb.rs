use std::{
    cell::{OnceCell, RefCell},
    fs::File,
    os::unix::fs::FileExt,
    rc::Rc,
};

use anyhow::Result;
use bytemuck::{bytes_of, from_bytes};

use crate::{
    CODEXFS_BLKSIZ_BITS, CODEXFS_MAGIC, CODEXFS_SUPERBLK_OFF, CodexFsSuperBlock,
    buffer::{BufferType, get_mut_bufmgr},
    ino_t,
    inode::Inode,
};

#[derive(Debug)]
pub struct SuperBlock {
    pub ino: ino_t,
    pub start_off: u64,
    pub img_file: File,
    root: OnceCell<Rc<RefCell<Inode>>>,
}

impl SuperBlock {
    fn new(img_file: File) -> Self {
        Self {
            ino: 0,
            start_off: 0,
            img_file,
            root: OnceCell::new(),
        }
    }

    pub fn set_root(&mut self, root: Rc<RefCell<Inode>>) {
        self.root.set(root).unwrap();
    }

    pub fn get_root(&self) -> &Rc<RefCell<Inode>> {
        self.root.get().unwrap()
    }

    pub fn get_ino_and_inc(&mut self) -> ino_t {
        let ino = self.ino;
        self.ino += 1;
        ino
    }

    pub fn get_start_off(&self) -> u64 {
        self.start_off
    }

    pub fn set_start_off(&mut self, off: u64) {
        self.start_off = off
    }

    pub fn inc_start_off(&mut self, inc: u64) {
        self.start_off += inc
    }
}

impl From<&SuperBlock> for CodexFsSuperBlock {
    fn from(sb: &SuperBlock) -> Self {
        Self {
            magic: CODEXFS_MAGIC,
            checksum: 0,
            blkszbits: CODEXFS_BLKSIZ_BITS,
            root_nid: sb.get_root().borrow().common.cf_nid,
            inos: sb.ino,
            blocks: 0,
            reserved: [0; _],
        }
    }
}

static mut SUPER_BLOCK: OnceCell<SuperBlock> = OnceCell::new();

pub fn set_sb(img_file: File) {
    unsafe { SUPER_BLOCK.set(SuperBlock::new(img_file)).unwrap() }
}

pub fn get_sb() -> &'static SuperBlock {
    unsafe { SUPER_BLOCK.get().unwrap() }
}

pub fn get_mut_sb() -> &'static mut SuperBlock {
    unsafe { SUPER_BLOCK.get_mut().unwrap() }
}

pub fn fuse_load_super_block() -> Result<()> {
    let mut sb_buf = [0; size_of::<CodexFsSuperBlock>()];
    get_sb()
        .img_file
        .read_exact_at(&mut sb_buf, CODEXFS_SUPERBLK_OFF)?;
    let codexfs_sb: &CodexFsSuperBlock = from_bytes(&sb_buf);
    let magic = codexfs_sb.magic;
    assert_eq!(magic, CODEXFS_MAGIC);
    let sb = get_mut_sb();
    sb.set_root(Rc::new(RefCell::new(Inode::load_from_nid(
        codexfs_sb.root_nid,
    )?)));
    Ok(())
}

pub fn mkfs_balloc_super_block() {
    let pos = get_mut_bufmgr().balloc(size_of::<CodexFsSuperBlock>() as _, BufferType::Meta);
    assert_eq!(pos, CODEXFS_SUPERBLK_OFF);
}

pub fn mkfs_dump_super_block() -> Result<()> {
    let codexfs_sb = CodexFsSuperBlock::from(get_sb());
    get_sb()
        .img_file
        .write_all_at(bytes_of(&codexfs_sb), CODEXFS_SUPERBLK_OFF)?;
    Ok(())
}

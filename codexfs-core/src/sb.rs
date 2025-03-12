use std::{cell::OnceCell, fs::File, os::unix::fs::FileExt};

use anyhow::{Ok, Result};
use bytemuck::{bytes_of, from_bytes};

use crate::{
    CODEXFS_BLKSIZ_BITS, CODEXFS_MAGIC, CODEXFS_SUPERBLK_OFF, CodexFsSuperBlock,
    buffer::{BufferType, get_bufmgr_mut},
    ino_t,
    inode::{Inode, InodeHandle},
};

#[derive(Debug)]
pub struct SuperBlock {
    pub ino: ino_t,
    pub img_file: File,
    root: OnceCell<InodeHandle>,
    pub end_data_blk_id: u32,
    pub end_data_blk_sz: u16,
}

impl SuperBlock {
    fn new(img_file: File) -> Self {
        Self {
            ino: 0,
            end_data_blk_id: 0,
            end_data_blk_sz: 0,
            img_file,
            root: OnceCell::new(),
        }
    }

    pub fn from_codexfs_sb(&mut self, codexfs_sb: &CodexFsSuperBlock) -> Result<()> {
        let root = Inode::load_from_nid(codexfs_sb.root_nid)?;
        self.root.set(root).unwrap();
        self.end_data_blk_id = codexfs_sb.end_data_blk_id;
        self.end_data_blk_sz = codexfs_sb.end_data_blk_sz;
        Ok(())
    }

    pub fn set_root(&self, root: InodeHandle) {
        self.root.set(root).unwrap()
    }

    pub fn get_root(&self) -> &InodeHandle {
        self.root.get().unwrap()
    }

    pub fn get_ino_and_inc(&mut self) -> ino_t {
        let ino = self.ino;
        self.ino += 1;
        ino
    }
}

impl From<&SuperBlock> for CodexFsSuperBlock {
    fn from(sb: &SuperBlock) -> Self {
        Self {
            magic: CODEXFS_MAGIC,
            checksum: 0,
            blkszbits: CODEXFS_BLKSIZ_BITS,
            root_nid: sb.get_root().meta().inner.borrow().nid,
            inos: sb.ino,
            blocks: 0,
            reserved: [0; _],
            end_data_blk_id: sb.end_data_blk_id,
            end_data_blk_sz: sb.end_data_blk_sz,
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

pub fn get_sb_mut() -> &'static mut SuperBlock {
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
    let sb = get_sb_mut();
    sb.from_codexfs_sb(codexfs_sb)?;
    Ok(())
}

pub fn mkfs_balloc_super_block() {
    let pos = get_bufmgr_mut().balloc(size_of::<CodexFsSuperBlock>() as _, BufferType::Meta);
    assert_eq!(pos, CODEXFS_SUPERBLK_OFF);
}

pub fn mkfs_dump_super_block() -> Result<()> {
    let codexfs_sb = CodexFsSuperBlock::from(get_sb());
    get_sb()
        .img_file
        .write_all_at(bytes_of(&codexfs_sb), CODEXFS_SUPERBLK_OFF)?;
    Ok(())
}

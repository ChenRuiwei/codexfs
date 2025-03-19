use std::{cell::OnceCell, fs::File, os::unix::fs::FileExt};

use anyhow::{Ok, Result};
use bytemuck::{bytes_of, from_bytes};

use crate::{
    CODEXFS_MAGIC, CODEXFS_SUPERBLK_OFF, CodexFsFlags, CodexFsInode, CodexFsSuperBlock, blk_size_t,
    blk_t,
    buffer::{BufferType, get_bufmgr_mut},
    ino_t,
    inode::{Inode, InodeHandle},
};

#[derive(Debug, Default)]
pub struct SuperBlock {
    pub islot_bits: u8,
    pub blksz_bits: u8,
    pub ino: ino_t,
    pub img_file: Option<File>,
    root: Option<InodeHandle>,
    pub end_data_blk_id: blk_t,
    pub end_data_blk_sz: blk_size_t,
    pub compress: bool,
}

impl SuperBlock {
    pub fn new(img_file: File, blksz_bits: u8) -> Self {
        let islot_bits = size_of::<CodexFsInode>().ilog2() as _;
        assert_eq!(
            2_u8.pow(islot_bits as _) as usize,
            size_of::<CodexFsInode>()
        );
        Self {
            img_file: Some(img_file),
            root: None,
            islot_bits,
            blksz_bits,
            ..Default::default()
        }
    }

    pub fn from_codexfs_sb(&mut self, codexfs_sb: &CodexFsSuperBlock) -> Result<()> {
        let root = Inode::load_from_nid(codexfs_sb.root_nid)?;
        self.set_root(root);
        self.end_data_blk_id = codexfs_sb.end_data_blk_id;
        self.end_data_blk_sz = codexfs_sb.end_data_blk_sz;
        self.islot_bits = codexfs_sb.islot_bits;
        self.blksz_bits = codexfs_sb.blksz_bits;
        self.compress = codexfs_sb.flags.contains(CodexFsFlags::CODEXFS_COMPRESSED);
        Ok(())
    }

    pub fn blksz(&self) -> blk_size_t {
        1 << self.blksz_bits
    }

    pub fn islotsz(&self) -> u8 {
        assert_eq!(1 << self.islot_bits, size_of::<CodexFsInode>());
        1 << self.islot_bits
    }

    pub fn set_root(&mut self, root: InodeHandle) {
        self.root = Some(root)
    }

    pub fn root(&self) -> &InodeHandle {
        self.root.as_ref().unwrap()
    }

    pub fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        self.img_file.as_ref().unwrap().read_exact_at(buf, offset)?;
        Ok(())
    }

    pub fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        self.img_file.as_ref().unwrap().write_all_at(buf, offset)?;
        Ok(())
    }

    pub fn get_ino_and_inc(&mut self) -> ino_t {
        let ino = self.ino;
        self.ino += 1;
        ino
    }
}

impl From<&SuperBlock> for CodexFsSuperBlock {
    fn from(sb: &SuperBlock) -> Self {
        let flags = if sb.compress {
            CodexFsFlags::CODEXFS_COMPRESSED
        } else {
            CodexFsFlags::empty()
        };
        Self {
            magic: CODEXFS_MAGIC,
            checksum: 0,
            blksz_bits: sb.blksz_bits,
            root_nid: sb.root().meta().inner.borrow().nid,
            inos: sb.ino,
            blocks: 0,
            reserved: [0; _],
            end_data_blk_id: sb.end_data_blk_id,
            end_data_blk_sz: sb.end_data_blk_sz,
            islot_bits: sb.islot_bits,
            flags,
        }
    }
}

static mut SUPER_BLOCK: OnceCell<SuperBlock> = OnceCell::new();

pub fn set_sb(sb: SuperBlock) {
    unsafe { SUPER_BLOCK.set(sb).unwrap() }
}

pub fn get_sb() -> &'static SuperBlock {
    unsafe { SUPER_BLOCK.get().unwrap() }
}

pub fn get_sb_mut() -> &'static mut SuperBlock {
    unsafe { SUPER_BLOCK.get_mut().unwrap() }
}

pub fn fuse_load_super_block(img_file: File) -> Result<()> {
    set_sb(SuperBlock::new(img_file, 0));
    let mut sb_buf = [0; size_of::<CodexFsSuperBlock>()];
    get_sb().read_exact_at(&mut sb_buf, CODEXFS_SUPERBLK_OFF)?;
    let codexfs_sb: &CodexFsSuperBlock = from_bytes(&sb_buf);
    let magic = codexfs_sb.magic;
    assert_eq!(magic, CODEXFS_MAGIC);
    get_sb_mut().from_codexfs_sb(codexfs_sb)?;
    Ok(())
}

pub fn mkfs_balloc_super_block() {
    let pos = get_bufmgr_mut().balloc(size_of::<CodexFsSuperBlock>() as _, BufferType::Meta);
    assert_eq!(pos, CODEXFS_SUPERBLK_OFF);
}

pub fn mkfs_dump_super_block() -> Result<()> {
    let codexfs_sb = CodexFsSuperBlock::from(get_sb());
    get_sb().write_all_at(bytes_of(&codexfs_sb), CODEXFS_SUPERBLK_OFF)?;
    Ok(())
}

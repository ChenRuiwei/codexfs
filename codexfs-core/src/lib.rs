#![feature(once_cell_get_mut)]
#![allow(static_mut_refs)]

pub mod inode;
pub mod sb;
pub mod utils;

use std::fs::FileType;

use bitflags::bitflags;
use bytemuck::{Pod, Zeroable};

pub const CODEXFS_MAGIC: u32 = 0x114514;

pub const CODEXFS_BLKSIZ_BITS: u8 = 12;
pub const CODEXFS_BLKSIZ: u16 = 1 << CODEXFS_BLKSIZ_BITS;
pub const CODEXFS_SUPERBLK_OFF: u64 = 0;
pub const CODEXFS_ISLOT_BITS: u64 = 6;

pub fn codexfs_blknr(addr: u64) -> u64 {
    addr >> CODEXFS_BLKSIZ_BITS
}

pub fn codexfs_blkoff(addr: u64) -> u16 {
    addr as u16 & (CODEXFS_BLKSIZ - 1)
}

pub fn codexfs_nid(addr: u64) -> u64 {
    addr >> CODEXFS_ISLOT_BITS
}

// codexfs on-disk super block (currently 128 bytes)
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsSuperBlock {
    pub magic: u32,    // file system magic number
    pub checksum: u32, // crc32c(super_block)
    pub blkszbits: u8, // filesystem block size in bit shift
    pub root_nid: u64, // nid of root directory
    pub inos: u32,     // total valid ino # (== f_files - f_favail)

    pub blocks: u32, // used for statfs
    pub reserved: [u8; 103],
}

// CODEXFS inode datalayout (i_format in on-disk inode):
// 0 - uncompressed flat inode without tail-packing inline data:
// 1 - compressed inode with non-compact indexes:
// 2 - uncompressed flat inode with tail-packing inline data:
// 3 - compressed inode with compact indexes:
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Pod, Zeroable)]
#[repr(transparent)]
pub struct CodexFsInodeFormat(u16);

bitflags! {
    impl CodexFsInodeFormat: u16 {
        const CODEXFS_INODE_FLAT_PLAIN			= 1 << 0;
        const CODEXFS_INODE_COMPRESSED_FULL		= 1 << 1;
        const CODEXFS_INODE_FLAT_INLINE         = 1 << 2;
        const CODEXFS_INODE_COMPRESSED_COMPACT  = 1 << 3;
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsInode {
    pub format: CodexFsInodeFormat,
    pub mode: u32,
    pub nlink: u16,
    pub size: u64,
    pub blknr: u64,
    pub blkoff: u16,
    pub ino: u32,
    pub uid: u32,
    pub gid: u32,
    pub reserved: [u8; 26], // reserved
}

#[derive(Clone, Copy, Debug, Zeroable)]
#[repr(u8)]
pub enum CodexFsFileType {
    Unknown,
    File,
    Dir,
    CharDev,
    BlkDev,
    Fifo,
    Sock,
    Symlink,
}

unsafe impl Pod for CodexFsFileType {}

impl From<FileType> for CodexFsFileType {
    fn from(val: FileType) -> Self {
        if val.is_dir() {
            CodexFsFileType::Dir
        } else if val.is_file() {
            CodexFsFileType::File
        } else if val.is_symlink() {
            CodexFsFileType::Symlink
        } else {
            CodexFsFileType::Unknown
        }
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsDirent {
    pub nid: u64,                   // node number
    pub nameoff: u16,               // start offset of file name
    pub file_type: CodexFsFileType, // file type
    pub reserved: u8,               // reserved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ondisk_layout_definitions() {
        assert_eq!(size_of::<CodexFsSuperBlock>(), 128);
        assert_eq!(size_of::<CodexFsInode>(), 1 << CODEXFS_ISLOT_BITS);
        assert_eq!(size_of::<CodexFsDirent>(), 12);
    }
}

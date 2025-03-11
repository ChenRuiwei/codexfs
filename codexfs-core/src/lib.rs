#![feature(once_cell_get_mut)]
#![feature(generic_arg_infer)]
#![allow(static_mut_refs)]
#![feature(vec_push_within_capacity)]
#![feature(string_from_utf8_lossy_owned)]
#![allow(non_camel_case_types)]

pub mod buffer;
pub mod compress;
pub mod inode;
pub mod sb;
pub mod utils;

use std::{fmt::Debug, os::unix::fs::FileTypeExt};

use anyhow::Result;
use bitflags::bitflags;
use bytemuck::{Pod, Zeroable};
use libc::{S_IFBLK, S_IFCHR, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK};
use utils::round_up;

type gid_t = u16;
type uid_t = u16;
type mode_t = u16;
type ino_t = u32;
type nid_t = u64;
type blk_t = u32;
type blk_size_t = u16;
type blk_off_t = blk_size_t;
type off_t = u64;
type size_t = u32;

pub const CODEXFS_MAGIC: u32 = 114514;

pub const CODEXFS_BLKSIZ_BITS: u8 = 12;
pub const CODEXFS_BLKSIZ: blk_size_t = 1 << CODEXFS_BLKSIZ_BITS;
pub const CODEXFS_SUPERBLK_OFF: u64 = 0;
pub const CODEXFS_ISLOT_BITS: u64 = 6;

pub fn addr_to_blk_id(addr: u64) -> blk_t {
    (addr >> CODEXFS_BLKSIZ_BITS) as _
}

pub fn addr_to_blk_off(addr: u64) -> u16 {
    addr as u16 & (CODEXFS_BLKSIZ - 1)
}

pub fn blk_id_to_addr(blk_id: blk_t) -> u64 {
    (blk_id as u64) << CODEXFS_BLKSIZ_BITS
}

pub fn addr_to_nid(addr: u64) -> u64 {
    assert_eq!(addr, round_up(addr, CODEXFS_ISLOT_BITS));
    addr >> CODEXFS_ISLOT_BITS
}

pub fn nid_to_inode_off(nid: nid_t) -> u64 {
    nid << CODEXFS_ISLOT_BITS
}

pub fn nid_to_inode_meta_off(nid: nid_t) -> u64 {
    (nid + 1) << CODEXFS_ISLOT_BITS
}

// codexfs on-disk super block (currently 128 bytes)
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsSuperBlock {
    pub magic: u32,      // file system magic number
    pub checksum: u32,   // crc32c(super_block)
    pub blkszbits: u8,   // filesystem block size in bit shift
    pub root_nid: nid_t, // nid of root directory
    pub inos: ino_t,     // total valid ino # (== f_files - f_favail)

    pub blocks: u32, // used for statfs
    pub end_data_blk_id: blk_t,
    pub end_data_blk_sz: blk_size_t,
    pub reserved: [u8; 97],
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
    pub mode: mode_t,
    pub nlink: u16,
    pub size: size_t,
    pub ino: ino_t,
    pub uid: uid_t,
    pub gid: gid_t,
    pub blk_id: blk_t,
    pub blks: u16,
    pub reserved: [u8; 40],
}

#[derive(Clone, Copy, Debug, Zeroable, PartialEq, Eq)]
#[repr(u8)]
pub enum CodexFsFileType {
    File,
    Dir,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Symlink,
}

unsafe impl Pod for CodexFsFileType {}

impl CodexFsFileType {
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }

    pub const fn is_symlink(self) -> bool {
        matches!(self, Self::Symlink)
    }

    pub const fn is_block_device(self) -> bool {
        matches!(self, Self::BlockDevice)
    }

    pub const fn is_char_device(self) -> bool {
        matches!(self, Self::CharDevice)
    }

    pub const fn is_fifo(self) -> bool {
        matches!(self, Self::Fifo)
    }

    pub const fn is_socket(self) -> bool {
        matches!(self, Self::Socket)
    }
}

impl From<std::fs::FileType> for CodexFsFileType {
    fn from(val: std::fs::FileType) -> Self {
        if val.is_dir() {
            CodexFsFileType::Dir
        } else if val.is_file() {
            CodexFsFileType::File
        } else if val.is_char_device() {
            CodexFsFileType::CharDevice
        } else if val.is_block_device() {
            CodexFsFileType::BlockDevice
        } else if val.is_fifo() {
            CodexFsFileType::Fifo
        } else if val.is_socket() {
            CodexFsFileType::Socket
        } else if val.is_symlink() {
            CodexFsFileType::Symlink
        } else {
            panic!()
        }
    }
}

impl From<mode_t> for CodexFsFileType {
    fn from(val: mode_t) -> Self {
        match (val as u32) & S_IFMT {
            S_IFREG => CodexFsFileType::File,
            S_IFDIR => CodexFsFileType::Dir,
            S_IFCHR => CodexFsFileType::CharDevice,
            S_IFBLK => CodexFsFileType::BlockDevice,
            S_IFSOCK => CodexFsFileType::Socket,
            S_IFLNK => CodexFsFileType::Symlink,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsDirent {
    pub nid: nid_t,                 // node number
    pub nameoff: u16,               // start offset of file name
    pub file_type: CodexFsFileType, // file type
    pub reserved: u8,               // reserved
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct CodexFsExtent {
    off: u32,      // offset in file
    frag_off: u32, // offset in decompressed fragment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ondisk_layout_definitions() {
        assert_eq!(size_of::<CodexFsSuperBlock>(), 128);
        assert_eq!(size_of::<CodexFsInode>(), 1 << CODEXFS_ISLOT_BITS);
        assert_eq!(size_of::<CodexFsDirent>(), 12);
        assert_eq!(size_of::<CodexFsExtent>(), 8);
    }
}

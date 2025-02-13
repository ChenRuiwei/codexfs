#![feature(once_cell_get_mut)]
#![allow(static_mut_refs)]

pub mod inode;
pub mod sb;
pub mod utils;

use std::{fs::FileType, os::unix::fs::FileTypeExt};

use bitflags::bitflags;
use bytemuck::{Pod, Zeroable};
use libc::{
    S_IFBLK, S_IFCHR, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK, gid_t, ino_t, mode_t, uid_t,
};

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
    pub inos: ino_t,   // total valid ino # (== f_files - f_favail)

    pub blocks: u32, // used for statfs
    pub reserved: [u8; 99],
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
    pub size: u64,
    pub blknr: u64,
    pub blkoff: u16,
    pub ino: ino_t,
    pub uid: uid_t,
    pub gid: gid_t,
    pub reserved: [u8; 22], // reserved
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum CodexFsFileType {
    Unknown,
    File,
    Dir,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Symlink,
}

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

impl From<FileType> for CodexFsFileType {
    fn from(val: FileType) -> Self {
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
            CodexFsFileType::Unknown
        }
    }
}

impl From<mode_t> for CodexFsFileType {
    fn from(val: mode_t) -> Self {
        match val & S_IFMT {
            S_IFREG => CodexFsFileType::File,
            S_IFDIR => CodexFsFileType::Dir,
            S_IFCHR => CodexFsFileType::CharDevice,
            S_IFBLK => CodexFsFileType::BlockDevice,
            S_IFSOCK => CodexFsFileType::Socket,
            S_IFLNK => CodexFsFileType::Symlink,
            _ => CodexFsFileType::Unknown,
        }
    }
}

impl From<CodexFsDirentFileType> for CodexFsFileType {
    fn from(val: CodexFsDirentFileType) -> Self {
        let val = val.0;
        if val == CODEXFS_FT_UNKNOWN {
            CodexFsFileType::Unknown
        } else if val == CODEXFS_FT_REG_FILE {
            CodexFsFileType::File
        } else if val == CODEXFS_FT_DIR {
            CodexFsFileType::Dir
        } else if val == CODEXFS_FT_CHRDEV {
            CodexFsFileType::CharDevice
        } else if val == CODEXFS_FT_BLKDEV {
            CodexFsFileType::BlockDevice
        } else if val == CODEXFS_FT_SOCK {
            CodexFsFileType::Socket
        } else if val == CODEXFS_FT_SYMLINK {
            CodexFsFileType::Symlink
        } else {
            panic!("unknown file type");
        }
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(transparent)]
pub struct CodexFsDirentFileType(u8);

const CODEXFS_FT_UNKNOWN: u8 = 0;
const CODEXFS_FT_REG_FILE: u8 = 1;
const CODEXFS_FT_DIR: u8 = 2;
const CODEXFS_FT_CHRDEV: u8 = 3;
const CODEXFS_FT_BLKDEV: u8 = 4;
const CODEXFS_FT_FIFO: u8 = 5;
const CODEXFS_FT_SOCK: u8 = 6;
const CODEXFS_FT_SYMLINK: u8 = 7;
const CODEXFS_FT_MAX: u8 = 8;

impl From<CodexFsFileType> for CodexFsDirentFileType {
    fn from(val: CodexFsFileType) -> Self {
        Self(match val {
            CodexFsFileType::Unknown => CODEXFS_FT_UNKNOWN,
            CodexFsFileType::File => CODEXFS_FT_REG_FILE,
            CodexFsFileType::Dir => CODEXFS_FT_DIR,
            CodexFsFileType::CharDevice => CODEXFS_FT_CHRDEV,
            CodexFsFileType::BlockDevice => CODEXFS_FT_BLKDEV,
            CodexFsFileType::Fifo => CODEXFS_FT_FIFO,
            CodexFsFileType::Socket => CODEXFS_FT_SOCK,
            CodexFsFileType::Symlink => CODEXFS_FT_SYMLINK,
        })
    }
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct CodexFsDirent {
    pub nid: u64,                         // node number
    pub nameoff: u16,                     // start offset of file name
    pub file_type: CodexFsDirentFileType, // file type
    pub reserved: u8,                     // reserved
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

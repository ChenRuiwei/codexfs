use std::{
    cell::{OnceCell, RefCell},
    fs::File,
    io,
    os::unix::fs::FileExt,
    path::Path,
    rc::Rc,
};

use crate::{
    CODEXFS_BLKSIZ, CODEXFS_BLKSIZ_BITS, CODEXFS_MAGIC, CODEXFS_SUPERBLK_OFF, CodexFsSuperBlock,
    inode::Inode, utils::round_up,
};

#[derive(Debug)]
pub struct SuperBlock {
    pub ino: u32,
    pub start_off: u64,
    pub img_file: File,
    root: OnceCell<Rc<RefCell<Inode>>>,
}

impl SuperBlock {
    fn new(img_file: File) -> Self {
        Self {
            ino: 0,
            start_off: round_up(
                CODEXFS_SUPERBLK_OFF + size_of::<CodexFsSuperBlock>() as u64,
                CODEXFS_BLKSIZ as u64,
            ),
            img_file,
            root: OnceCell::new(),
        }
    }

    pub fn init_root(&mut self, root: Rc<RefCell<Inode>>) {
        self.root.set(root);
    }

    pub fn get_root(&self) -> &Rc<RefCell<Inode>> {
        self.root.get().unwrap()
    }

    pub fn get_ino_and_inc(&mut self) -> u32 {
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
            root_nid: sb.get_root().borrow().cf_nid,
            inos: sb.ino,
            blocks: 0,
            reserved: [0; 103],
        }
    }
}

static mut SUPER_BLOCK: OnceCell<SuperBlock> = OnceCell::new();

pub fn set_sb(img_path: &Path) {
    let img_file = File::create(img_path).unwrap();
    unsafe { SUPER_BLOCK.set(SuperBlock::new(img_file)).unwrap() }
}

pub fn get_sb() -> &'static SuperBlock {
    unsafe { SUPER_BLOCK.get().unwrap() }
}

pub fn get_mut_sb() -> &'static mut SuperBlock {
    unsafe { SUPER_BLOCK.get_mut().unwrap() }
}

pub fn dump_super_block() -> io::Result<()> {
    let codexfs_sb = CodexFsSuperBlock::from(get_sb());
    get_sb()
        .img_file
        .write_all_at(codexfs_sb.to_bytes(), CODEXFS_SUPERBLK_OFF)?;
    Ok(())
}

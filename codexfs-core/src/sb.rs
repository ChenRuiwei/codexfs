use std::{
    cell::{OnceCell, RefCell},
    fs::File,
    io,
    os::unix::fs::FileExt,
    path::Path,
    rc::Rc,
};

use bytemuck::{bytes_of, from_bytes};

use crate::{
    CODEXFS_BLKSIZ, CODEXFS_BLKSIZ_BITS, CODEXFS_MAGIC, CODEXFS_SUPERBLK_OFF, CodexFsSuperBlock,
    ino_t, inode::Inode, utils::round_up,
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
            start_off: round_up(
                CODEXFS_SUPERBLK_OFF + size_of::<CodexFsSuperBlock>() as u64,
                CODEXFS_BLKSIZ as u64,
            ),
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
            root_nid: sb.get_root().borrow().cf_nid,
            inos: sb.ino,
            blocks: 0,
            reserved: [0; _],
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

pub fn load_super_block() -> io::Result<()> {
    let mut buf = [0; size_of::<CodexFsSuperBlock>()];
    get_sb()
        .img_file
        .read_exact_at(&mut buf, CODEXFS_SUPERBLK_OFF)?;
    let codexfs_sb: &CodexFsSuperBlock = from_bytes::<_>(&buf);
    let sb = get_mut_sb();

    Ok(())
}

pub fn mkfs_dump_super_block() -> io::Result<()> {
    let codexfs_sb = CodexFsSuperBlock::from(get_sb());
    get_sb()
        .img_file
        .write_all_at(bytes_of(&codexfs_sb), CODEXFS_SUPERBLK_OFF)?;
    Path::new("");
    Ok(())
}

use std::{
    cell::{OnceCell, RefCell},
    cmp::{self, Ordering},
    ops::{Deref, DerefMut},
    rc::Rc,
};

use crate::{
    CodexFsInode, blk_id_to_addr, blk_off_t, blk_size_t, blk_t, sb::get_sb, utils::round_up,
};

pub enum BufferType {
    Meta,
    Inode,
    Data,
}

pub fn get_align(btype: BufferType) -> blk_size_t {
    match btype {
        BufferType::Meta => 1,
        BufferType::Inode => size_of::<CodexFsInode>() as _,
        BufferType::Data => get_sb().blksz(),
    }
}

pub fn get_bufmgr_mut() -> &'static mut BufferManager {
    static mut BUFFER_MANAGER: OnceCell<BufferManager> = OnceCell::new();
    unsafe { BUFFER_MANAGER.get_mut_or_init(BufferManager::new) }
}

pub struct BufferBlockTable(
    Vec<Vec<Rc<RefCell<BufferBlock>>>>, // index means for unused size
);

impl Deref for BufferBlockTable {
    type Target = Vec<Vec<Rc<RefCell<BufferBlock>>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BufferBlockTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl BufferBlockTable {
    fn new() -> Self {
        Self(vec![Vec::new(); get_sb().blksz() as usize + 1])
    }
}

pub struct BufferManager {
    pub table: BufferBlockTable,
    pub tail_blk: Rc<RefCell<BufferBlock>>,
}

impl BufferManager {
    fn new() -> Self {
        let buf_blk = Rc::new(RefCell::new(BufferBlock::new(0)));
        let mut buf_mgr = Self {
            table: BufferBlockTable::new(),
            tail_blk: buf_blk.clone(),
        };
        buf_mgr.push_block(buf_blk);
        buf_mgr
    }

    pub fn balloc(&mut self, size: u64, btype: BufferType) -> u64 {
        let alignment = get_align(btype);
        assert!(alignment <= get_sb().blksz());
        let aligned_size = round_up(size, alignment as _);

        if let Some(addr) = self.bfind(aligned_size, alignment) {
            return addr;
        }

        self.balloc_contig(aligned_size, alignment)
    }

    fn bfind(&mut self, aligned_size: u64, align: blk_size_t) -> Option<u64> {
        assert_eq!(aligned_size, round_up(aligned_size, align as _));
        if aligned_size > get_sb().blksz() as _ {
            return None;
        }
        let size = aligned_size as _;
        for i in size..get_sb().blksz() + 1 {
            let i = i as usize;
            if self.table[i].is_empty() {
                continue;
            }
            let buf_blk = self.table[i].pop().unwrap();
            let addr = buf_blk.borrow().addr();
            buf_blk.borrow_mut().blk_off += size;
            self.push_block(buf_blk);
            return Some(addr);
        }
        None
    }

    fn balloc_contig(&mut self, aligned_size: u64, align: blk_size_t) -> u64 {
        assert_eq!(aligned_size, round_up(aligned_size, align as _));
        let aligned_off = round_up(self.tail_blk.borrow().blk_off, align);
        let (addr, mut size_left) = match aligned_off.cmp(&get_sb().blksz()) {
            Ordering::Less => {
                assert!((aligned_off as u64 + aligned_size) > get_sb().blksz() as u64);
                let addr = self.tail_blk.borrow().addr();
                let size_left = aligned_size - ((get_sb().blksz() - aligned_off) as u64);
                (addr, size_left)
            }
            Ordering::Equal => {
                let addr = blk_id_to_addr(self.tail_blk_id() + 1);
                let size_left = aligned_size;
                (addr, size_left)
            }
            Ordering::Greater => panic!(),
        };

        while size_left > 0 {
            let mut buf_blk = BufferBlock::new(self.tail_blk_id() + 1);
            buf_blk.blk_off = cmp::min(get_sb().blksz() as u64, size_left) as _;
            size_left -= buf_blk.blk_off as u64;
            let buf_blk = Rc::new(RefCell::new(buf_blk));
            self.tail_blk = buf_blk.clone();
            self.push_block(buf_blk);
        }

        addr
    }

    pub fn tail_blk_id(&self) -> blk_t {
        self.tail_blk.borrow().blk_id
    }

    fn push_block(&mut self, buf_blk: Rc<RefCell<BufferBlock>>) {
        let off = buf_blk.borrow().blk_off;
        self.table[(get_sb().blksz() - off) as usize].push(buf_blk);
    }
}

pub struct BufferBlock {
    pub blk_id: blk_t,
    pub blk_off: blk_off_t,
}

impl BufferBlock {
    fn new(blk_id: blk_t) -> Self {
        Self { blk_id, blk_off: 0 }
    }

    fn addr(&self) -> u64 {
        blk_id_to_addr(self.blk_id) + (self.blk_off as u64)
    }
}

use std::{
    cell::{OnceCell, Ref, RefCell},
    collections::HashMap,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::Result;
use bytemuck::{bytes_of, checked::from_bytes};

use crate::{
    CODEXFS_ISLOT_BITS, CodexFsDirent, CodexFsFileType, CodexFsInode, CodexFsInodeFormat,
    codexfs_nid, gid_t, ino_t, mode_t,
    sb::{get_mut_sb, get_sb},
    uid_t,
};

type InodeTable = HashMap<ino_t, Rc<RefCell<Inode>>>;

fn get_mut_inode_table() -> &'static mut InodeTable {
    static mut FILE_NODE_TABLE: OnceCell<InodeTable> = OnceCell::new();
    unsafe { FILE_NODE_TABLE.get_mut_or_init(HashMap::new) }
}

fn get_inode(ino: ino_t) -> Option<&'static Rc<RefCell<Inode>>> {
    get_mut_inode_table().get(&ino)
}

fn insert_inode(ino: ino_t, node: Rc<RefCell<Inode>>) {
    get_mut_inode_table().insert(ino, node);
}

#[derive(Debug)]
pub struct Inode {
    pub path: Option<PathBuf>,
    pub file_type: CodexFsFileType,
    pub size: u64,
    pub dentries: Vec<Dentry>, // TODO: handle dot and dotdot

    // Fields prefixed with "cf" (for codexfs) are unrelated to the original file system.
    pub cf_blkpos: Option<u64>,
    pub cf_ino: ino_t,
    pub cf_uid: uid_t,
    pub cf_gid: gid_t,
    pub cf_mode: mode_t,
    pub cf_nid: u64,
    pub cf_nlink: u16,
}

#[derive(Debug)]
pub struct Dentry {
    pub path: Option<PathBuf>,
    pub file_name: String,
    pub file_type: CodexFsFileType,
    pub inode: Rc<RefCell<Inode>>,
}

impl Inode {
    fn new(path: &Path) -> Self {
        let metadata = path.metadata().unwrap();
        Self {
            path: Some(path.into()),
            file_type: metadata.file_type().into(),
            dentries: Vec::new(),
            size: metadata.len(),
            cf_blkpos: None,
            cf_nlink: 0,
            cf_ino: get_mut_sb().get_ino_and_inc(),
            cf_gid: metadata.gid(),
            cf_uid: metadata.uid(),
            cf_nid: 0,
            cf_mode: metadata.mode(),
        }
    }

    fn new_dir(path: &Path) -> Self {
        let metadata = path.metadata().unwrap();
        Self {
            path: Some(path.into()),
            file_type: metadata.file_type().into(),
            dentries: Vec::new(),
            cf_blkpos: None,
            size: metadata.len(),
            cf_nlink: 2,
            cf_ino: get_mut_sb().get_ino_and_inc(),
            cf_nid: 0,
            cf_mode: metadata.mode(),
            cf_gid: metadata.gid(),
            cf_uid: metadata.uid(),
        }
    }

    pub fn from_nid(nid: u64) -> Result<Self> {
        let mut inode_buf = [0; size_of::<CodexFsInode>()];
        get_sb()
            .img_file
            .read_exact_at(&mut inode_buf, nid >> CODEXFS_ISLOT_BITS)?;
        let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);
        Ok(Self::from_codexfs_inode(codexfs_inode, nid))
    }

    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self {
        Self {
            path: None,
            file_type: CodexFsFileType::from(codexfs_inode.mode),
            size: codexfs_inode.size,
            dentries: Vec::new(),
            cf_blkpos: if codexfs_inode.blkpos != 0 {
                Some(codexfs_inode.blkpos)
            } else {
                None
            },
            cf_ino: codexfs_inode.ino,
            cf_uid: codexfs_inode.uid,
            cf_gid: codexfs_inode.gid,
            cf_mode: codexfs_inode.mode,
            cf_nid: nid,
            cf_nlink: codexfs_inode.nlink,
        }
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref().unwrap()
    }

    fn inc_nlink(&mut self) {
        self.cf_nlink += 1
    }

    fn add_dentry(&mut self, dentry: Dentry) {
        self.dentries.push(dentry);
    }

    pub fn print_recursive(&self, depth: usize) {
        let indent = "\t".repeat(depth);
        println!(
            "{}Inode: {:?}, {:?}, size={}, nlink={}",
            indent,
            self.path(),
            self.file_type,
            self.size,
            self.cf_nlink
        );

        for dentry in &self.dentries {
            dentry.inode.borrow().print_recursive(depth + 1);
        }
    }
}

impl Dentry {
    fn new(path: &Path, node: Rc<RefCell<Inode>>) -> Self {
        let metadata = path.metadata().unwrap();
        Dentry {
            path: Some(path.into()),
            file_name: path.file_name().unwrap().to_string_lossy().to_string(),
            file_type: metadata.file_type().into(),
            inode: node,
        }
    }

    fn file_name(&self) -> &str {
        &self.file_name
    }
}

impl From<&Ref<'_, Inode>> for CodexFsInode {
    fn from(node: &Ref<'_, Inode>) -> Self {
        Self {
            format: CodexFsInodeFormat::CODEXFS_INODE_FLAT_PLAIN,
            mode: node.cf_mode,
            nlink: node.cf_nlink,
            size: node.size,
            blkpos: node.cf_blkpos.unwrap_or(0),
            ino: node.cf_ino,
            uid: node.cf_uid,
            gid: node.cf_gid,
            reserved: [0; _],
        }
    }
}

impl From<&Dentry> for CodexFsDirent {
    fn from(dentry: &Dentry) -> Self {
        Self {
            nid: dentry.inode.borrow().cf_nid,
            nameoff: 0,
            file_type: dentry.file_type.into(),
            reserved: 0,
        }
    }
}

pub fn mkfs_load_inode_tree(path: &Path) -> Result<Inode, std::io::Error> {
    assert!(path.is_dir());

    let mut root = Inode::new_dir(path);

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let entry_ino = entry.metadata()?.ino();

        if entry_path.is_dir() {
            let child = mkfs_load_inode_tree(&entry_path)?;
            let child = Rc::new(RefCell::new(child));
            insert_inode(entry_ino, child.clone());
            let child_dentry = Dentry::new(&entry_path, child);
            root.inc_nlink();
            root.add_dentry(child_dentry);
        } else {
            let child = get_inode(entry_ino).cloned().unwrap_or_else(|| {
                let child = Inode::new(&entry_path);
                assert!(child.file_type.is_file());
                Rc::new(RefCell::new(child))
            });
            child.borrow_mut().inc_nlink();
            insert_inode(entry_ino, child.clone());
            let child_dentry = Dentry::new(&entry_path, child);
            root.add_dentry(child_dentry);
        }

        println!("{}", entry.path().to_string_lossy());
    }

    Ok(root)
}

pub fn mkfs_calc_inode_off(root: &Rc<RefCell<Inode>>) {
    for dentry in root.borrow_mut().dentries.iter_mut() {
        let child = &dentry.inode;
        if child.borrow().file_type.is_dir() {
            mkfs_calc_inode_off(child);
        } else {
            let mut child = child.borrow_mut();
            assert!(child.file_type.is_file());
            let start_off = get_sb().get_start_off();
            if child.cf_blkpos.is_none() {
                child.cf_blkpos = Some(start_off);
            }
            get_mut_sb().set_start_off(start_off + child.size);
        }
    }
}

// FIXME: dirent off should be calculated
pub fn mkfs_dump_inode_tree(node: &Rc<RefCell<Inode>>) -> io::Result<()> {
    let sb = get_mut_sb();
    let file_type = node.borrow().file_type;

    if file_type.is_dir() {
        {
            let node_ref = node.borrow();
            for dentry in node_ref.dentries.iter() {
                let child = &dentry.inode;
                mkfs_dump_inode_tree(child)?;
            }
            let inode = CodexFsInode::from(&node_ref);
            sb.img_file
                .write_all_at(bytes_of(&inode), sb.get_start_off())?;
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
        {
            let mut node_mut = node.borrow_mut();
            node_mut.cf_nid = codexfs_nid(sb.get_start_off());
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
        {
            let node_ref = node.borrow();
            for dentry in node_ref.dentries.iter() {
                let codexfs_dirent = CodexFsDirent::from(dentry);
                sb.img_file
                    .write_all_at(bytes_of(&codexfs_dirent), sb.get_start_off())?;
                sb.inc_start_off(size_of::<CodexFsDirent>() as u64);
            }
            for dentry in node_ref.dentries.iter() {
                sb.img_file
                    .write_all_at(dentry.file_name().as_bytes(), sb.get_start_off())?;
                sb.inc_start_off(dentry.file_name().len() as u64);
            }
        }
    } else if file_type.is_file() {
        {
            let node_ref = node.borrow();
            let mut file = File::open(node_ref.path())?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            sb.img_file
                .write_all_at(&content, node_ref.cf_blkpos.unwrap())?;

            let codexfs_inode = CodexFsInode::from(&node_ref);
            sb.img_file
                .write_all_at(bytes_of(&codexfs_inode), sb.get_start_off())?;
        }
        {
            let mut node_mut = node.borrow_mut();
            node_mut.cf_nid = codexfs_nid(sb.get_start_off());
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
    } else {
        todo!()
    }

    Ok(())
}

pub fn load_inode(nid: u64) -> io::Result<Inode> {
    let mut buf = [0; size_of::<CodexFsInode>()];
    get_sb()
        .img_file
        .read_exact_at(&mut buf, nid >> CODEXFS_ISLOT_BITS)?;
    let codexfs_inode: &CodexFsInode = from_bytes(&buf);
    let file_type: CodexFsFileType = codexfs_inode.mode.into();
    let inode = match file_type {
        CodexFsFileType::Unknown => todo!(),
        CodexFsFileType::File => Inode {
            path: None,
            file_type,
            size: codexfs_inode.size,
            dentries: Vec::new(),
            cf_blkpos: Some(codexfs_inode.blkpos),
            cf_ino: codexfs_inode.ino,
            cf_uid: codexfs_inode.uid,
            cf_gid: codexfs_inode.gid,
            cf_mode: codexfs_inode.mode,
            cf_nid: nid,
            cf_nlink: codexfs_inode.nlink,
        },
        CodexFsFileType::Dir => Inode {
            path: None,
            file_type,
            size: codexfs_inode.size,
            dentries: Vec::new(),
            cf_blkpos: Some(codexfs_inode.blkpos),
            cf_ino: codexfs_inode.ino,
            cf_uid: codexfs_inode.uid,
            cf_gid: codexfs_inode.gid,
            cf_mode: codexfs_inode.mode,
            cf_nid: nid,
            cf_nlink: codexfs_inode.nlink,
        },
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
        CodexFsFileType::Symlink => todo!(),
    };
    Ok(inode)
}

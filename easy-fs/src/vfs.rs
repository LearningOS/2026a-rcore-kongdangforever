use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};

/// Metadata needed by the kernel's `fstat` implementation.
pub struct InodeStat {
    /// Inode number in the filesystem.
    pub inode_id: u32,
    /// Whether this inode represents a directory.
    pub is_dir: bool,
    /// Number of hard links to this inode.
    pub nlink: u32,
}
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Find inode under a disk inode by name
    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                ))
            })
        })
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        if name.is_empty() || name.len() > 27 {
            return None;
        }
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            // Reuse a directory-entry tombstone left by unlink when possible.
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let mut empty_index = None;
            let mut current = DirEntry::empty();
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(i * DIRENT_SZ, current.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if current.name().is_empty() {
                    empty_index = Some(i);
                    break;
                }
            }
            let entry_index = empty_index.unwrap_or(file_count);
            let new_size = (file_count + 1) * DIRENT_SZ;
            if empty_index.is_none() {
                self.increase_size(new_size as u32, root_inode, &mut fs);
            }
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                entry_index * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
        // release efs lock automatically by compiler
    }
    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if !dirent.name().is_empty() {
                    v.push(String::from(dirent.name()));
                }
            }
            v
        })
    }

    /// Create `new_name` as another directory entry for `old_name`.
    pub fn link(&self, old_name: &str, new_name: &str) -> bool {
        if old_name.is_empty() || new_name.is_empty() || old_name == new_name || new_name.len() > 27
        {
            return false;
        }

        let mut fs = self.fs.lock();
        let (old_inode_id, empty_index, file_count) = match self.read_disk_inode(|root_inode| {
            if !root_inode.is_dir() || self.find_inode_id(new_name, root_inode).is_some() {
                return None;
            }
            let old_inode_id = self.find_inode_id(old_name, root_inode)?;
            let file_count = root_inode.size as usize / DIRENT_SZ;
            let mut empty_index = None;
            let mut dirent = DirEntry::empty();
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if dirent.name().is_empty() {
                    empty_index = Some(i);
                    break;
                }
            }
            Some((old_inode_id, empty_index, file_count))
        }) {
            Some(result) => result,
            None => return false,
        };

        let (inode_block_id, inode_block_offset) = fs.get_disk_inode_pos(old_inode_id);
        let link_count_increased =
            get_block_cache(inode_block_id as usize, Arc::clone(&self.block_device))
                .lock()
                .modify(inode_block_offset, |inode: &mut DiskInode| {
                    inode.increase_nlink()
                });
        if !link_count_increased {
            return false;
        }

        self.modify_disk_inode(|root_inode| {
            let entry_index = empty_index.unwrap_or(file_count);
            if empty_index.is_none() {
                self.increase_size(((file_count + 1) * DIRENT_SZ) as u32, root_inode, &mut fs);
            }
            let dirent = DirEntry::new(new_name, old_inode_id);
            assert_eq!(
                root_inode.write_at(
                    entry_index * DIRENT_SZ,
                    dirent.as_bytes(),
                    &self.block_device,
                ),
                DIRENT_SZ,
            );
        });
        block_cache_sync_all();
        true
    }

    /// Remove a directory entry and reclaim the inode when its last link goes away.
    pub fn unlink(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        let mut fs = self.fs.lock();
        let (entry_index, inode_id) = match self.read_disk_inode(|root_inode| {
            if !root_inode.is_dir() {
                return None;
            }
            let file_count = root_inode.size as usize / DIRENT_SZ;
            let mut dirent = DirEntry::empty();
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if dirent.name() == name {
                    return Some((i, dirent.inode_id()));
                }
            }
            None
        }) {
            Some(result) => result,
            None => return false,
        };

        // Leave an empty reusable entry instead of shrinking the root directory;
        // this avoids leaking or overwriting its last data block.
        self.modify_disk_inode(|root_inode| {
            let empty = DirEntry::empty();
            assert_eq!(
                root_inode.write_at(
                    entry_index * DIRENT_SZ,
                    empty.as_bytes(),
                    &self.block_device,
                ),
                DIRENT_SZ,
            );
        });

        let (inode_block_id, inode_block_offset) = fs.get_disk_inode_pos(inode_id);
        let data_blocks = get_block_cache(inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(inode_block_offset, |inode: &mut DiskInode| {
                match inode.decrease_nlink() {
                    Some(0) => Some(inode.clear_size(&self.block_device)),
                    Some(_) => None,
                    None => None,
                }
            });

        if let Some(data_blocks) = data_blocks {
            for block_id in data_blocks {
                fs.dealloc_data(block_id);
            }
            fs.dealloc_inode(inode_id);
        }
        block_cache_sync_all();
        true
    }

    /// Return the inode number, type and hard-link count.
    pub fn stat(&self) -> InodeStat {
        let fs = self.fs.lock();
        let inode_id = fs.get_disk_inode_id(self.block_id as u32, self.block_offset);
        self.read_disk_inode(|inode| InodeStat {
            inode_id,
            is_dir: inode.is_dir(),
            nlink: inode.nlink(),
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}

//! Use the in-tree RAMFS nodes with the kernel's synchronization primitives.

use alloc::sync::Arc;
use axfs_vfs::{VfsNodeRef, VfsOps, VfsResult};
use axsync::Mutex;

#[path = "../../../../axfs_ramfs/src/dir.rs"]
mod dir;
#[path = "../../../../axfs_ramfs/src/file.rs"]
mod file;

pub struct RamFileSystem {
    root: Arc<dir::DirNode>,
    // Directory parent pointers are weak, so keep the mount parent alive.
    parent: Mutex<Option<VfsNodeRef>>,
}

impl RamFileSystem {
    pub fn new() -> Self {
        Self {
            root: dir::DirNode::new(None),
            parent: Mutex::new(None),
        }
    }
}

impl VfsOps for RamFileSystem {
    fn mount(&self, _path: &str, mount_point: VfsNodeRef) -> VfsResult {
        let parent = mount_point.parent();
        self.root.set_parent(parent.as_ref());
        *self.parent.lock() = parent;
        Ok(())
    }

    fn root_dir(&self) -> VfsNodeRef {
        self.root.clone()
    }
}

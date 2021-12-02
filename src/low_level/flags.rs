use std::fmt::{Display, Formatter};

use crate::sys;

bitflags::bitflags! {
    /// Known `FSEvents` event flags.
    #[repr(C)]
    pub struct StreamFlags: u32 {
        const NONE = sys::kFSEventStreamEventFlagNone;
        const MUST_SCAN_SUBDIRS = sys::kFSEventStreamEventFlagMustScanSubDirs;
        const USER_DROPPED = sys::kFSEventStreamEventFlagUserDropped;
        const KERNEL_DROPPED = sys::kFSEventStreamEventFlagKernelDropped;
        const IDS_WRAPPED = sys::kFSEventStreamEventFlagEventIdsWrapped;
        const HISTORY_DONE = sys::kFSEventStreamEventFlagHistoryDone;
        const ROOT_CHANGED = sys::kFSEventStreamEventFlagRootChanged;
        const MOUNT = sys::kFSEventStreamEventFlagMount;
        const UNMOUNT = sys::kFSEventStreamEventFlagUnmount;
        const ITEM_CREATED = sys::kFSEventStreamEventFlagItemCreated;
        const ITEM_REMOVED = sys::kFSEventStreamEventFlagItemRemoved;
        const INODE_META_MOD = sys::kFSEventStreamEventFlagItemInodeMetaMod;
        const ITEM_RENAMED = sys::kFSEventStreamEventFlagItemRenamed;
        const ITEM_MODIFIED = sys::kFSEventStreamEventFlagItemModified;
        const FINDER_INFO_MOD = sys::kFSEventStreamEventFlagItemFinderInfoMod;
        const ITEM_CHANGE_OWNER = sys::kFSEventStreamEventFlagItemChangeOwner;
        const ITEM_XATTR_MOD = sys::kFSEventStreamEventFlagItemXattrMod;
        const IS_FILE = sys::kFSEventStreamEventFlagItemIsFile;
        const IS_DIR = sys::kFSEventStreamEventFlagItemIsDir;
        const IS_SYMLINK = sys::kFSEventStreamEventFlagItemIsSymlink;
        const OWN_EVENT = sys::kFSEventStreamEventFlagOwnEvent;
        const IS_HARDLINK = sys::kFSEventStreamEventFlagItemIsHardlink;
        const IS_LAST_HARDLINK = sys::kFSEventStreamEventFlagItemIsLastHardlink;
        const ITEM_CLONED = sys::kFSEventStreamEventFlagItemCloned;
    }
}

impl Display for StreamFlags {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        if self.contains(Self::MUST_SCAN_SUBDIRS) {
            let _d = write!(f, "MUST_SCAN_SUBDIRS ");
        }
        if self.contains(Self::USER_DROPPED) {
            let _d = write!(f, "USER_DROPPED ");
        }
        if self.contains(Self::KERNEL_DROPPED) {
            let _d = write!(f, "KERNEL_DROPPED ");
        }
        if self.contains(Self::IDS_WRAPPED) {
            let _d = write!(f, "IDS_WRAPPED ");
        }
        if self.contains(Self::HISTORY_DONE) {
            let _d = write!(f, "HISTORY_DONE ");
        }
        if self.contains(Self::ROOT_CHANGED) {
            let _d = write!(f, "ROOT_CHANGED ");
        }
        if self.contains(Self::MOUNT) {
            let _d = write!(f, "MOUNT ");
        }
        if self.contains(Self::UNMOUNT) {
            let _d = write!(f, "UNMOUNT ");
        }
        if self.contains(Self::ITEM_CREATED) {
            let _d = write!(f, "ITEM_CREATED ");
        }
        if self.contains(Self::ITEM_REMOVED) {
            let _d = write!(f, "ITEM_REMOVED ");
        }
        if self.contains(Self::INODE_META_MOD) {
            let _d = write!(f, "INODE_META_MOD ");
        }
        if self.contains(Self::ITEM_RENAMED) {
            let _d = write!(f, "ITEM_RENAMED ");
        }
        if self.contains(Self::ITEM_MODIFIED) {
            let _d = write!(f, "ITEM_MODIFIED ");
        }
        if self.contains(Self::FINDER_INFO_MOD) {
            let _d = write!(f, "FINDER_INFO_MOD ");
        }
        if self.contains(Self::ITEM_CHANGE_OWNER) {
            let _d = write!(f, "ITEM_CHANGE_OWNER ");
        }
        if self.contains(Self::ITEM_XATTR_MOD) {
            let _d = write!(f, "ITEM_XATTR_MOD ");
        }
        if self.contains(Self::IS_FILE) {
            let _d = write!(f, "IS_FILE ");
        }
        if self.contains(Self::IS_DIR) {
            let _d = write!(f, "IS_DIR ");
        }
        if self.contains(Self::IS_SYMLINK) {
            let _d = write!(f, "IS_SYMLINK ");
        }
        if self.contains(Self::OWN_EVENT) {
            let _d = write!(f, "OWN_EVENT ");
        }
        if self.contains(Self::IS_LAST_HARDLINK) {
            let _d = write!(f, "IS_LAST_HARDLINK ");
        }
        if self.contains(Self::IS_HARDLINK) {
            let _d = write!(f, "IS_HARDLINK ");
        }
        if self.contains(Self::ITEM_CLONED) {
            let _d = write!(f, "ITEM_CLONED ");
        }
        write!(f, "")
    }
}

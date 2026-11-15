//! Pure userspace ext2/3/4 support (read + build), no mount and no root.

pub mod disk;
pub mod read;
pub mod write;

pub use read::Ext4;
pub use write::{build_tree, BuildOptions};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perm::{Entry, FileContexts, FsConfig, Kind, MetaDb};
    use std::fs;

    #[test]
    fn build_then_read_tree() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        fs::create_dir_all(root.join("system/bin")).unwrap();
        fs::create_dir_all(root.join("system/etc")).unwrap();
        fs::write(root.join("system/bin/sh"), b"\x7fELF fake sh").unwrap();
        fs::write(root.join("system/etc/build.prop"), b"ro.foo=1\n").unwrap();

        let fsc = FsConfig::parse(
            "/system 0 0 0755\n/system/bin 0 2000 0755\n/system/bin/sh 0 2000 0755\n/system/etc 0 0 0755\n/system/etc/build.prop 0 0 0644\n",
        )
        .unwrap();
        let fc = FileContexts::parse(
            "/system(/.*)? u:object_r:system_file:s0\n/system/bin/sh u:object_r:shell_exec:s0\n",
        )
        .unwrap();
        let mut db = MetaDb::from_dir(&root, &fsc, &fc).unwrap();
        // MetaDb::from_dir reports paths relative to `root`, so /system/... is
        // already correct; make sure the root entry exists as well.
        db.insert(crate::perm::default_entry("/", Kind::Dir, 0));

        let img = d.path().join("system.img");
        build_tree(
            &root,
            &db,
            &img,
            &BuildOptions {
                size: 16 * 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap();

        let mut fs_img = Ext4::open(&img).unwrap();
        assert_eq!(fs_img.sb.block_size, 4096);
        let ino = fs_img.lookup("/system/bin/sh").unwrap();
        let inode = fs_img.inode(ino).unwrap();
        assert_eq!(inode.uid, 0);
        assert_eq!(inode.gid, 2000);
        assert_eq!(inode.mode & 0o7777, 0o755);
        let data = fs_img.read_data(&inode).unwrap();
        assert_eq!(data, b"\x7fELF fake sh");
        let ctx = fs_img.selinux_context(&inode).unwrap().unwrap();
        assert_eq!(ctx, "u:object_r:shell_exec:s0");

        let entries = fs_img.read_dir(2).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"system"));

        // extract and compare
        let out = d.path().join("out");
        let meta2 = fs_img.extract_tree(&out).unwrap();
        let e = meta2.get("/system/bin/sh").unwrap();
        assert_eq!(e.gid, 2000);
        assert_eq!(e.mode, 0o755);
        assert_eq!(
            fs::read(out.join("system/bin/sh")).unwrap(),
            b"\x7fELF fake sh"
        );
    }

    #[test]
    fn symlink_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("r");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/busybox"), b"busybox").unwrap();
        let mut db = MetaDb::new();
        db.insert(Entry {
            kind: Kind::Dir,
            path: "/bin".into(),
            uid: 0,
            gid: 0,
            mode: 0o755,
            ctx: "u:object_r:system_file:s0".into(),
            caps: 0,
            size: 0,
            mtime: 0,
            link: None,            ino: None,
        });
        db.insert(Entry {
            kind: Kind::File,
            path: "/bin/busybox".into(),
            uid: 0,
            gid: 0,
            mode: 0o755,
            ctx: "u:object_r:system_file:s0".into(),
            caps: 0,
            size: 7,
            mtime: 0,
            link: None,            ino: None,
        });
        db.insert(Entry {
            kind: Kind::Link,
            path: "/bin/ls".into(),
            uid: 0,
            gid: 0,
            mode: 0o777,
            ctx: "u:object_r:system_file:s0".into(),
            caps: 0,
            size: 0,
            mtime: 0,
            link: Some("/bin/busybox".into()),            ino: None,
        });
        let img = d.path().join("t.img");
        build_tree(
            &root,
            &db,
            &img,
            &BuildOptions {
                size: 8 * 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
        let mut f = Ext4::open(&img).unwrap();
        let ino = f.lookup("/bin/ls").unwrap();
        let inode = f.inode(ino).unwrap();
        assert!(inode.is_symlink());
        assert_eq!(f.read_data(&inode).unwrap(), b"/bin/busybox");
    }
}

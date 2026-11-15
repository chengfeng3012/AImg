//! End-to-end CLI tests: every stage of the Windows toolchain is exercised
//! through the real binary, in the same order a ROM workflow would use them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_aimg")
}

fn run(args: &[&str]) -> String {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn aimg {:?}: {}", args, e));
    if !out.status.success() {
        panic!(
            "aimg {:?} failed:\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn write(p: &Path, data: &[u8]) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, data).unwrap();
}

fn sample_tree(root: &Path) {
    write(&root.join("system/bin/sh"), b"\x7fELF fake shell\n");
    write(&root.join("system/bin/toybox"), &vec![0x5au8; 9000]);
    write(&root.join("system/etc/build.prop"), b"ro.product.name=aimg\n");
    write(&root.join("system/etc/init/hw/init.rc"), b"on boot\n");
    write(&root.join("system/app/Foo/Foo.apk"), &vec![0x11u8; 40000]);
}

fn sample_configs(dir: &Path) -> (PathBuf, PathBuf) {
    let fsc = dir.join("fs_config.txt");
    let fc = dir.join("file_contexts");
    write(
        &fsc,
        b"# path uid gid mode [capabilities=0x..]\n\
/system 0 0 0755\n\
/system/bin 0 2000 0755\n\
/system/bin/sh 0 2000 0755\n\
/system/bin/toybox 0 2000 0755\n\
/system/etc 0 0 0755\n\
/system/etc/build.prop 0 0 0644\n\
/system/etc/init 0 0 0755\n\
/system/etc/init/hw 0 0 0755\n\
/system/etc/init/hw/init.rc 0 0 0750\n\
/system/app 0 0 0755\n\
/system/app/Foo 0 0 0755\n\
/system/app/Foo/Foo.apk 0 0 0644\n",
    );
    write(
        &fc,
        b"/system(/.*)? u:object_r:system_file:s0\n\
/system/bin/sh u:object_r:shell_exec:s0\n\
/system/bin/toybox u:object_r:toybox_exec:s0\n\
/system/app(/.*)? u:object_r:system_app_file:s0\n",
    );
    (fsc, fc)
}

#[test]
fn full_pipeline_ext4_sparse_super_payload_dat() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    sample_tree(&root);
    let (fsc, fc) = sample_configs(d.path());

    // 1. text-config driven metadata export
    let meta = d.path().join("meta.txt");
    run(&[
        "perm",
        "export",
        "--dir",
        root.to_str().unwrap(),
        "--out",
        meta.to_str().unwrap(),
        "--fs-config",
        fsc.to_str().unwrap(),
        "--file-contexts",
        fc.to_str().unwrap(),
    ]);
    let meta_text = fs::read_to_string(&meta).unwrap();
    assert!(meta_text.starts_with("# aimg-meta v1"));
    assert!(meta_text.contains("u:object_r:shell_exec:s0"));

    // 2. build the ext4 image from tree + meta
    let img = d.path().join("system.img");
    run(&[
        "ext4",
        "build",
        "--dir",
        root.to_str().unwrap(),
        "--meta",
        meta.to_str().unwrap(),
        "--out",
        img.to_str().unwrap(),
        "--size",
        "32M",
        "--label",
        "system",
    ]);
    let info = run(&["ext4", "info", img.to_str().unwrap()]);
    assert!(info.contains("4096"), "{}", info);
    assert!(info.contains("ext4"));

    let ls = run(&["ext4", "ls", img.to_str().unwrap()]);
    assert!(ls.contains("system/"), "{}", ls);

    // 3. extract back and verify content + metadata survived
    let out = d.path().join("out");
    run(&[
        "ext4",
        "extract",
        img.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(out.join("system/bin/sh")).unwrap(),
        b"\x7fELF fake shell\n"
    );
    assert_eq!(fs::read(out.join("system/bin/toybox")).unwrap().len(), 9000);
    let meta2 = fs::read_to_string(out.join("meta.txt")).unwrap();
    assert!(meta2.contains("/system/bin/sh\t0\t2000\t0755\tu:object_r:shell_exec:s0"));

    // 4. sparse round trip
    let simg = d.path().join("system.simg");
    run(&[
        "sparse",
        "from-raw",
        img.to_str().unwrap(),
        "--out",
        simg.to_str().unwrap(),
    ]);
    let raw2 = d.path().join("system.raw");
    run(&[
        "sparse",
        "to-raw",
        simg.to_str().unwrap(),
        "--out",
        raw2.to_str().unwrap(),
    ]);
    assert_eq!(fs::read(&img).unwrap(), fs::read(&raw2).unwrap());

    // 5. super.img build with the real partition image, then unpack it
    let super_img = d.path().join("super.img");
    run(&[
        "super",
        "make",
        "--out",
        super_img.to_str().unwrap(),
        "--partition",
        &format!("system_a={}", img.display()),
        "--size",
        "64M",
    ]);
    let dump = run(&["super", "dump", super_img.to_str().unwrap()]);
    assert!(dump.contains("system_a"), "{}", dump);
    let sup_out = d.path().join("super_out");
    run(&[
        "super",
        "unpack",
        super_img.to_str().unwrap(),
        "--out",
        sup_out.to_str().unwrap(),
        "--slot",
        "0",
    ]);
    let got = fs::read(sup_out.join("system.img")).unwrap();
    let want = fs::read(&img).unwrap();
    assert_eq!(&got[..want.len()], &want[..]);

    // 6. payload.bin build + extract
    let payload = d.path().join("payload.bin");
    run(&[
        "payload",
        "make",
        "--out",
        payload.to_str().unwrap(),
        "--partition",
        &format!("system={}", img.display()),
        "--chunk-size",
        "1048576",
    ]);
    let pinfo = run(&["payload", "info", payload.to_str().unwrap()]);
    assert!(pinfo.contains("system"), "{}", pinfo);
    let pout = d.path().join("payload_out");
    run(&[
        "payload",
        "extract",
        payload.to_str().unwrap(),
        "--out",
        pout.to_str().unwrap(),
    ]);
    assert_eq!(fs::read(pout.join("system.img")).unwrap(), want);

    // 7. .dat split + merge
    let dat_out = d.path().join("dat");
    run(&[
        "dat",
        "split",
        img.to_str().unwrap(),
        "--out",
        dat_out.to_str().unwrap(),
        "--prefix",
        "system",
    ]);
    let merged = d.path().join("merged.img");
    run(&[
        "dat",
        "merge",
        dat_out.join("system.transfer.list").to_str().unwrap(),
        dat_out.join("system.new.dat").to_str().unwrap(),
        "--out",
        merged.to_str().unwrap(),
    ]);
    assert_eq!(fs::read(&merged).unwrap(), want);
}

#[test]
fn perm_meta_tsv_is_stable() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("r");
    write(&root.join("a/b.txt"), b"x");
    let (fsc, fc) = sample_configs(d.path());
    let meta = d.path().join("m.txt");
    run(&[
        "perm",
        "export",
        "--dir",
        root.to_str().unwrap(),
        "--out",
        meta.to_str().unwrap(),
        "--fs-config",
        fsc.to_str().unwrap(),
        "--file-contexts",
        fc.to_str().unwrap(),
    ]);
    let text = fs::read_to_string(&meta).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines.len(), 3, "{}", text); // /, /a and /a/b.txt
    // re-parsing must be lossless
    let show = run(&[
        "perm",
        "show",
        "--meta",
        meta.to_str().unwrap(),
        "/a/b.txt",
    ]);
    assert!(show.contains("mode=0644"), "{}", show);
}

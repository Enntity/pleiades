// SPDX-License-Identifier: AGPL-3.0-only
//! install.sh against a local file:// mirror via SHRUG_BASE_URL.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
use std::path::PathBuf;
use std::process::Command;

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn mirror() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shrug-mirror-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A fake "shrug" that just prints a version line.
    let fake = dir.join("fake-shrug");
    std::fs::write(&fake, "#!/bin/sh\necho shrug 0.1.0 (catalog serial 1)\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let tb = dir.join("shrug-aarch64-unknown-linux-musl.tar.gz");
    let st = Command::new("tar")
        .args(["-czf"])
        .arg(&tb)
        .arg("-C")
        .arg(&dir)
        .arg("--transform")
        .arg("s/fake-shrug/shrug/")
        .arg("fake-shrug")
        .status()
        .unwrap();
    assert!(st.success());
    let sum = Command::new("sha256sum").arg(&tb).output().unwrap();
    let mut sums = String::from_utf8(sum.stdout).unwrap();
    // sha256sum prints the full path; keep just the tarball name.
    let name = "shrug-aarch64-unknown-linux-musl.tar.gz";
    let hex: String = sums.split_whitespace().next().unwrap().to_string();
    sums = format!("{hex}  {name}\n");
    std::fs::write(dir.join("SHA256SUMS"), sums).unwrap();
    dir
}

#[test]
#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
fn refuses_non_aarch64() {
    let out = Command::new("sh").arg("install.sh").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains("Shrug runs on NVIDIA DGX Spark (Linux aarch64)"),
        "{err}"
    );
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn installs_from_mirror() {
    let dir = mirror();
    let dest = dir.join("bin");
    let out = Command::new("sh")
        .arg("install.sh")
        .env("SHRUG_BASE_URL", format!("file://{}", dir.display()))
        .env("SHRUG_INSTALL_DIR", &dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Command::new(dest.join("shrug"))
        .arg("--version")
        .output()
        .unwrap();
    let s = String::from_utf8(v.stdout).unwrap();
    assert!(s.contains("catalog serial 1"), "{s}");
    std::fs::remove_dir_all(&dir).ok();
}
